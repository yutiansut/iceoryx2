// Copyright (c) 2024 Contributors to the Eclipse Foundation
//
// See the NOTICE file(s) distributed with this work for additional
// information regarding copyright ownership.
//
// This program and the accompanying materials are made available under the
// terms of the Apache Software License 2.0 which is available at
// https://www.apache.org/licenses/LICENSE-2.0, or the MIT license
// which is available at https://opensource.org/licenses/MIT.
//
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A SlotMap is a container that has a static unique key for every stored value. Adding or
//! removing values to the SlotMap do not change the unique key of the remaining values.
//! Multiple variationes of that container are available.
//!
//!  * [`SlotMap`](crate::slotmap::SlotMap), run-time fixed-size slotmap that is not shared-memory
//!    compatible since the memory resides in the heap.
//!  * [`FixedSizeSlotMap`](crate::slotmap::FixedSizeSlotMap), compile-time fixed-size slotmap that
//!    is self-contained and shared-memory compatible.
//!  * [`RelocatableSlotMap`](crate::slotmap::RelocatableSlotMap), run-time fixed-size slotmap that
//!    is shared-memory compatible.
//!
//! # User Examples
//!
//! ```
//! use iceoryx2_bb_container::slotmap::FixedSizeSlotMap;
//!
//! const CAPACITY: usize = 123;
//! let mut slotmap = FixedSizeSlotMap::<u64, CAPACITY>::new();
//!
//! let key = slotmap.insert(78181).unwrap();
//!
//! println!("value: {:?}", slotmap.get(key));
//! ```

use crate::queue::details::MetaQueue;
use crate::vec::details::MetaVec;
use crate::{queue::RelocatableQueue, vec::RelocatableVec};
use iceoryx2_bb_elementary::bump_allocator::BumpAllocator;
use iceoryx2_bb_elementary::owning_pointer::OwningPointer;
use iceoryx2_bb_elementary::placement_default::PlacementDefault;
use iceoryx2_bb_elementary::pointer_trait::PointerTrait;
use iceoryx2_bb_elementary::relocatable_container::RelocatableContainer;
use iceoryx2_bb_elementary::relocatable_ptr::RelocatablePointer;
use iceoryx2_bb_log::fail;
use std::mem::MaybeUninit;

/// A key of a [`SlotMap`], [`RelocatableSlotMap`] or [`FixedSizeSlotMap`] that identifies a
/// value.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub struct SlotMapKey(usize);

impl SlotMapKey {
    /// Creates a new [`SlotMapKey`] with the specified value.
    pub fn new(value: usize) -> Self {
        Self(value)
    }

    /// Returns the underlying value of the [`SlotMapKey`].
    pub fn value(&self) -> usize {
        self.0
    }
}

/// A runtime fixed-size, non-shared memory compatible [`SlotMap`]. The [`SlotMap`]s memory resides
/// in the heap.
pub type SlotMap<T> = details::MetaSlotMap<
    T,
    OwningPointer<MaybeUninit<Option<T>>>,
    OwningPointer<MaybeUninit<usize>>,
>;

/// A runtime fixed-size, shared-memory compatible [`RelocatableSlotMap`].
pub type RelocatableSlotMap<T> = details::MetaSlotMap<
    T,
    RelocatablePointer<MaybeUninit<Option<T>>>,
    RelocatablePointer<MaybeUninit<usize>>,
>;

const INVALID_KEY: usize = usize::MAX;

#[doc(hidden)]
pub mod details {
    use super::*;

    /// The iterator of a [`SlotMap`], [`RelocatableSlotMap`] or [`FixedSizeSlotMap`].
    pub struct Iter<
        'slotmap,
        T,
        DataPtrType: PointerTrait<MaybeUninit<Option<T>>>,
        IdxPtrType: PointerTrait<MaybeUninit<usize>>,
    > {
        slotmap: &'slotmap MetaSlotMap<T, DataPtrType, IdxPtrType>,
        key: SlotMapKey,
    }

    pub type OwningIter<'slotmap, T> =
        Iter<'slotmap, T, OwningPointer<MaybeUninit<Option<T>>>, OwningPointer<MaybeUninit<usize>>>;
    pub type RelocatableIter<'slotmap, T> = Iter<
        'slotmap,
        T,
        RelocatablePointer<MaybeUninit<Option<T>>>,
        RelocatablePointer<MaybeUninit<usize>>,
    >;

    impl<
            'slotmap,
            T,
            DataPtrType: PointerTrait<MaybeUninit<Option<T>>>,
            IdxPtrType: PointerTrait<MaybeUninit<usize>>,
        > Iterator for Iter<'slotmap, T, DataPtrType, IdxPtrType>
    {
        type Item = (SlotMapKey, &'slotmap T);

        fn next(&mut self) -> Option<Self::Item> {
            if let Some((key, value)) = self.slotmap.next(self.key) {
                self.key.0 = key.0 + 1;
                Some((key, value))
            } else {
                None
            }
        }
    }

    #[repr(C)]
    #[derive(Debug)]
    pub struct MetaSlotMap<
        T,
        DataPtrType: PointerTrait<MaybeUninit<Option<T>>>,
        IdxPtrType: PointerTrait<MaybeUninit<usize>>,
    > {
        idx_to_data: MetaVec<usize, IdxPtrType>,
        idx_to_data_next_free_index: MetaQueue<usize, IdxPtrType>,
        data: MetaVec<Option<T>, DataPtrType>,
        data_next_free_index: MetaQueue<usize, IdxPtrType>,
    }

    impl<
            T,
            DataPtrType: PointerTrait<MaybeUninit<Option<T>>>,
            IdxPtrType: PointerTrait<MaybeUninit<usize>>,
        > MetaSlotMap<T, DataPtrType, IdxPtrType>
    {
        fn next(&self, start: SlotMapKey) -> Option<(SlotMapKey, &T)> {
            let idx_to_data = &self.idx_to_data;

            for n in start.0..idx_to_data.len() {
                let data_idx = self.idx_to_data[n];
                if data_idx != INVALID_KEY {
                    return Some((
                        SlotMapKey(n),
                        self.data[data_idx].as_ref().expect(
                            "By contract, data contains a value when idx_to_data contains a value",
                        ),
                    ));
                }
            }

            None
        }

        pub(crate) unsafe fn initialize_data_structures(&mut self) {
            for n in 0..self.capacity_impl() {
                self.idx_to_data.push_impl(INVALID_KEY);
                self.data.push_impl(None);
                self.idx_to_data_next_free_index.push_impl(n);
                self.data_next_free_index.push_impl(n);
            }
        }

        pub(crate) unsafe fn iter_impl(&self) -> Iter<T, DataPtrType, IdxPtrType> {
            Iter {
                slotmap: self,
                key: SlotMapKey(0),
            }
        }

        pub(crate) unsafe fn contains_impl(&self, key: SlotMapKey) -> bool {
            self.idx_to_data[key.0] != INVALID_KEY
        }

        pub(crate) unsafe fn get_impl(&self, key: SlotMapKey) -> Option<&T> {
            match self.idx_to_data[key.0] {
                INVALID_KEY => None,
                n => Some(self.data[n].as_ref().expect(
                    "data and idx_to_data correspond and this value must be always available.",
                )),
            }
        }

        pub(crate) unsafe fn get_mut_impl(&mut self, key: SlotMapKey) -> Option<&mut T> {
            match self.idx_to_data[key.0] {
                INVALID_KEY => None,
                n => Some(self.data[n].as_mut().expect(
                    "data and idx_to_data correspond and this value must be always available.",
                )),
            }
        }

        pub(crate) unsafe fn insert_impl(&mut self, value: T) -> Option<SlotMapKey> {
            match self.idx_to_data_next_free_index.pop_impl() {
                None => None,
                Some(key) => {
                    let key = SlotMapKey(key);
                    self.insert_at_impl(key, value);
                    Some(key)
                }
            }
        }

        pub(crate) unsafe fn insert_at_impl(&mut self, key: SlotMapKey, value: T) -> bool {
            if key.0 > self.capacity_impl() {
                return false;
            }

            let data_idx = self.idx_to_data[key.0];
            if data_idx != INVALID_KEY {
                self.data[data_idx] = Some(value);
            } else {
                let n = self.data_next_free_index.pop_impl().expect("data and idx_to_data correspond and there must be always a free index available.");
                self.idx_to_data[key.0] = n;
                self.data[n] = Some(value);
            }

            true
        }

        pub(crate) unsafe fn remove_impl(&mut self, key: SlotMapKey) -> bool {
            if key.0 > self.idx_to_data.len() {
                return false;
            }

            let data_idx = self.idx_to_data[key.0];
            if data_idx != INVALID_KEY {
                self.data[data_idx].take();
                self.data_next_free_index.push_impl(data_idx);
                self.idx_to_data_next_free_index.push_impl(key.0);
                self.idx_to_data[key.0] = INVALID_KEY;
                true
            } else {
                false
            }
        }

        pub(crate) fn len_impl(&self) -> usize {
            self.capacity_impl() - self.idx_to_data_next_free_index.len()
        }

        pub(crate) fn capacity_impl(&self) -> usize {
            self.idx_to_data.capacity()
        }

        pub(crate) fn is_empty_impl(&self) -> bool {
            self.len_impl() == 0
        }

        pub(crate) fn is_full_impl(&self) -> bool {
            self.len_impl() == self.capacity_impl()
        }
    }

    impl<T> RelocatableContainer
        for MetaSlotMap<
            T,
            RelocatablePointer<MaybeUninit<Option<T>>>,
            RelocatablePointer<MaybeUninit<usize>>,
        >
    {
        unsafe fn new_uninit(capacity: usize) -> Self {
            Self {
                idx_to_data: RelocatableVec::new_uninit(capacity),
                idx_to_data_next_free_index: RelocatableQueue::new_uninit(capacity),
                data: RelocatableVec::new_uninit(capacity),
                data_next_free_index: RelocatableQueue::new_uninit(capacity),
            }
        }

        unsafe fn init<Allocator: iceoryx2_bb_elementary::allocator::BaseAllocator>(
            &mut self,
            allocator: &Allocator,
        ) -> Result<(), iceoryx2_bb_elementary::allocator::AllocationError> {
            let msg = "Unable to initialize RelocatableSlotMap";
            fail!(from "RelocatableSlotMap::init()",
                  when self.idx_to_data.init(allocator),
                  "{msg} since the underlying idx_to_data vector could not be initialized.");
            fail!(from "RelocatableSlotMap::init()",
                  when self.idx_to_data_next_free_index.init(allocator),
                  "{msg} since the underlying idx_to_data_next_free_index queue could not be initialized.");
            fail!(from "RelocatableSlotMap::init()",
                  when self.data.init(allocator),
                  "{msg} since the underlying data vector could not be initialized.");
            fail!(from "RelocatableSlotMap::init()",
                  when self.data_next_free_index.init(allocator),
                  "{msg} since the underlying data_next_free_index queue could not be initialized.");

            self.initialize_data_structures();
            Ok(())
        }

        fn memory_size(capacity: usize) -> usize {
            Self::const_memory_size(capacity)
        }
    }

    impl<T>
        MetaSlotMap<
            T,
            RelocatablePointer<MaybeUninit<Option<T>>>,
            RelocatablePointer<MaybeUninit<usize>>,
        >
    {
        /// Returns how many memory the [`RelocatableSlotMap`] will allocate from the allocator
        /// in [`RelocatableSlotMap::init()`].
        pub const fn const_memory_size(capacity: usize) -> usize {
            RelocatableVec::<usize>::const_memory_size(capacity)
                + RelocatableQueue::<usize>::const_memory_size(capacity)
                + RelocatableVec::<Option<T>>::const_memory_size(capacity)
                + RelocatableQueue::<usize>::const_memory_size(capacity)
        }
    }

    impl<T> MetaSlotMap<T, OwningPointer<MaybeUninit<Option<T>>>, OwningPointer<MaybeUninit<usize>>> {
        /// Creates a new runtime-fixed size [`SlotMap`] on the heap with the given capacity.
        pub fn new(capacity: usize) -> Self {
            let mut new_self = Self {
                idx_to_data: MetaVec::new(capacity),
                idx_to_data_next_free_index: MetaQueue::new(capacity),
                data: MetaVec::new(capacity),
                data_next_free_index: MetaQueue::new(capacity),
            };
            unsafe { new_self.initialize_data_structures() };
            new_self
        }

        /// Returns the [`Iter`]ator to iterate over all entries.
        pub fn iter(&self) -> OwningIter<T> {
            unsafe { self.iter_impl() }
        }

        /// Returns `true` if the provided `key` is contained, otherwise `false`.
        pub fn contains(&self, key: SlotMapKey) -> bool {
            unsafe { self.contains_impl(key) }
        }

        /// Returns a reference to the value stored under the given key. If there is no such key,
        /// [`None`] is returned.
        pub fn get(&self, key: SlotMapKey) -> Option<&T> {
            unsafe { self.get_impl(key) }
        }

        /// Returns a mutable reference to the value stored under the given key. If there is no
        /// such key, [`None`] is returned.
        pub fn get_mut(&mut self, key: SlotMapKey) -> Option<&mut T> {
            unsafe { self.get_mut_impl(key) }
        }

        /// Insert a value and returns the corresponding [`SlotMapKey`]. If the container is full
        /// [`None`] is returned.
        pub fn insert(&mut self, value: T) -> Option<SlotMapKey> {
            unsafe { self.insert_impl(value) }
        }

        /// Insert a value at the specified [`SlotMapKey`] and returns true.  If the provided key
        /// is out-of-bounds it returns `false` and adds nothing. If there is already a value
        /// stored at the `key`s index, the value is overridden with the provided value.
        pub fn insert_at(&mut self, key: SlotMapKey, value: T) -> bool {
            unsafe { self.insert_at_impl(key, value) }
        }

        /// Removes a value at the specified [`SlotMapKey`]. If there was no value corresponding
        /// to the [`SlotMapKey`] it returns false, otherwise true.
        pub fn remove(&mut self, key: SlotMapKey) -> bool {
            unsafe { self.remove_impl(key) }
        }

        /// Returns the number of stored values.
        pub fn len(&self) -> usize {
            self.len_impl()
        }

        /// Returns the capacity.
        pub fn capacity(&self) -> usize {
            self.capacity_impl()
        }

        /// Returns true if the container is empty, otherwise false.
        pub fn is_empty(&self) -> bool {
            self.is_empty_impl()
        }

        /// Returns true if the container is full, otherwise false.
        pub fn is_full(&self) -> bool {
            self.is_full_impl()
        }
    }

    impl<T>
        MetaSlotMap<
            T,
            RelocatablePointer<MaybeUninit<Option<T>>>,
            RelocatablePointer<MaybeUninit<usize>>,
        >
    {
        /// Returns the [`Iter`]ator to iterate over all entries.
        ///
        /// # Safety
        ///
        ///  * [`RelocatableSlotMap::init()`] must be called once before
        ///
        pub unsafe fn iter(&self) -> RelocatableIter<T> {
            self.iter_impl()
        }

        /// Returns `true` if the provided `key` is contained, otherwise `false`.
        ///
        /// # Safety
        ///
        ///  * [`RelocatableSlotMap::init()`] must be called once before
        ///
        pub unsafe fn contains(&self, key: SlotMapKey) -> bool {
            self.contains_impl(key)
        }

        /// Returns a reference to the value stored under the given key. If there is no such key,
        /// [`None`] is returned.
        ///
        /// # Safety
        ///
        ///  * [`RelocatableSlotMap::init()`] must be called once before
        ///
        pub unsafe fn get(&self, key: SlotMapKey) -> Option<&T> {
            self.get_impl(key)
        }

        /// Returns a mutable reference to the value stored under the given key. If there is no
        /// such key, [`None`] is returned.
        ///
        /// # Safety
        ///
        ///  * [`RelocatableSlotMap::init()`] must be called once before
        ///
        pub unsafe fn get_mut(&mut self, key: SlotMapKey) -> Option<&mut T> {
            self.get_mut_impl(key)
        }

        /// Insert a value and returns the corresponding [`SlotMapKey`]. If the container is full
        /// [`None`] is returned.
        ///
        /// # Safety
        ///
        ///  * [`RelocatableSlotMap::init()`] must be called once before
        ///
        pub unsafe fn insert(&mut self, value: T) -> Option<SlotMapKey> {
            self.insert_impl(value)
        }

        /// Insert a value at the specified [`SlotMapKey`] and returns true.  If the provided key
        /// is out-of-bounds it returns `false` and adds nothing. If there is already a value
        /// stored at the `key`s index, the value is overridden with the provided value.
        ///
        /// # Safety
        ///
        ///  * [`RelocatableSlotMap::init()`] must be called once before
        ///
        pub unsafe fn insert_at(&mut self, key: SlotMapKey, value: T) -> bool {
            self.insert_at_impl(key, value)
        }

        /// Removes a value at the specified [`SlotMapKey`]. If there was no value corresponding
        /// to the [`SlotMapKey`] it returns false, otherwise true.
        ///
        /// # Safety
        ///
        ///  * [`RelocatableSlotMap::init()`] must be called once before
        ///
        pub unsafe fn remove(&mut self, key: SlotMapKey) -> bool {
            self.remove_impl(key)
        }

        /// Returns the number of stored values.
        pub fn len(&self) -> usize {
            self.len_impl()
        }

        /// Returns the capacity.
        pub fn capacity(&self) -> usize {
            self.capacity_impl()
        }

        /// Returns true if the container is empty, otherwise false.
        pub fn is_empty(&self) -> bool {
            self.is_empty_impl()
        }

        /// Returns true if the container is full, otherwise false.
        pub fn is_full(&self) -> bool {
            self.is_full_impl()
        }
    }
}

/// A compile-time fixed-size, shared memory compatible [`FixedSizeSlotMap`].
#[repr(C)]
#[derive(Debug)]
pub struct FixedSizeSlotMap<T, const CAPACITY: usize> {
    state: RelocatableSlotMap<T>,
    _idx_to_data: [usize; CAPACITY],
    _idx_to_data_next_free_index: [usize; CAPACITY],
    _data: [Option<T>; CAPACITY],
    _data_next_free_index: [usize; CAPACITY],
}

impl<T, const CAPACITY: usize> PlacementDefault for FixedSizeSlotMap<T, CAPACITY> {
    unsafe fn placement_default(ptr: *mut Self) {
        let state_ptr = core::ptr::addr_of_mut!((*ptr).state);
        state_ptr.write(unsafe { RelocatableSlotMap::new_uninit(CAPACITY) });
        let allocator = BumpAllocator::new(core::ptr::addr_of!((*ptr)._data) as usize);
        (*ptr)
            .state
            .init(&allocator)
            .expect("All required memory is preallocated.");
    }
}

impl<T, const CAPACITY: usize> Default for FixedSizeSlotMap<T, CAPACITY> {
    fn default() -> Self {
        let mut new_self = Self {
            _idx_to_data: core::array::from_fn(|_| INVALID_KEY),
            _idx_to_data_next_free_index: core::array::from_fn(|_| 0),
            _data: core::array::from_fn(|_| None),
            _data_next_free_index: core::array::from_fn(|_| 0),
            state: unsafe { RelocatableSlotMap::new_uninit(CAPACITY) },
        };

        let allocator = BumpAllocator::new(core::ptr::addr_of!(new_self._idx_to_data) as usize);
        unsafe {
            new_self
                .state
                .init(&allocator)
                .expect("All required memory is preallocated.")
        };

        new_self
    }
}

impl<T, const CAPACITY: usize> FixedSizeSlotMap<T, CAPACITY> {
    /// Creates a new empty [`FixedSizeSlotMap`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the [`details::RelocatableIter`]ator to iterate over all entries.
    pub fn iter(&self) -> details::RelocatableIter<T> {
        unsafe { self.state.iter_impl() }
    }

    /// Returns `true` if the provided `key` is contained, otherwise `false`.
    pub fn contains(&self, key: SlotMapKey) -> bool {
        unsafe { self.state.contains_impl(key) }
    }

    /// Returns a reference to the value stored under the given key. If there is no such key,
    /// [`None`] is returned.
    pub fn get(&self, key: SlotMapKey) -> Option<&T> {
        unsafe { self.state.get_impl(key) }
    }

    /// Returns a mutable reference to the value stored under the given key. If there is no
    /// such key, [`None`] is returned.
    pub fn get_mut(&mut self, key: SlotMapKey) -> Option<&mut T> {
        unsafe { self.state.get_mut_impl(key) }
    }

    /// Insert a value and returns the corresponding [`SlotMapKey`]. If the container is full
    /// [`None`] is returned.
    pub fn insert(&mut self, value: T) -> Option<SlotMapKey> {
        unsafe { self.state.insert_impl(value) }
    }

    /// Insert a value at the specified [`SlotMapKey`] and returns true.  If the provided key
    /// is out-of-bounds it returns `false` and adds nothing. If there is already a value
    /// stored at the `key`s index, the value is overridden with the provided value.
    pub fn insert_at(&mut self, key: SlotMapKey, value: T) -> bool {
        unsafe { self.state.insert_at_impl(key, value) }
    }

    /// Removes a value at the specified [`SlotMapKey`]. If there was no value corresponding
    /// to the [`SlotMapKey`] it returns false, otherwise true.
    pub fn remove(&mut self, key: SlotMapKey) -> bool {
        unsafe { self.state.remove_impl(key) }
    }

    /// Returns the number of stored values.
    pub fn len(&self) -> usize {
        self.state.len_impl()
    }

    /// Returns the capacity.
    pub fn capacity(&self) -> usize {
        self.state.capacity_impl()
    }

    /// Returns true if the container is empty, otherwise false.
    pub fn is_empty(&self) -> bool {
        self.state.is_empty_impl()
    }

    /// Returns true if the container is full, otherwise false.
    pub fn is_full(&self) -> bool {
        self.state.is_full_impl()
    }
}
