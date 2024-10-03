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

#[generic_tests::define]
mod waitset {
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    use iceoryx2::port::listener::Listener;
    use iceoryx2::port::notifier::Notifier;
    use iceoryx2::port::waitset::WaitSetAttachmentError;
    use iceoryx2::prelude::{WaitSetBuilder, *};
    use iceoryx2_bb_posix::config::test_directory;
    use iceoryx2_bb_posix::directory::Directory;
    use iceoryx2_bb_posix::file::Permission;
    use iceoryx2_bb_posix::unix_datagram_socket::{
        UnixDatagramReceiver, UnixDatagramSender, UnixDatagramSenderBuilder,
    };
    use iceoryx2_bb_posix::{
        file_descriptor_set::SynchronousMultiplexing, unique_system_id::UniqueSystemId,
        unix_datagram_socket::UnixDatagramReceiverBuilder,
    };
    use iceoryx2_bb_system_types::file_path::*;
    use iceoryx2_bb_system_types::path::*;
    use iceoryx2_bb_testing::watchdog::Watchdog;
    use iceoryx2_bb_testing::{assert_that, test_fail};
    use iceoryx2_cal::event::Event;

    const TIMEOUT: Duration = Duration::from_millis(100);

    fn generate_name() -> ServiceName {
        ServiceName::new(&format!(
            "waitset_tests_{}",
            UniqueSystemId::new().unwrap().value()
        ))
        .unwrap()
    }

    fn generate_uds_name() -> FilePath {
        let mut path = test_directory();
        Directory::create(&path, Permission::OWNER_ALL).unwrap();
        let _ = path.add_path_entry(
            &Path::new(
                &format!("waitset_tests_{}", UniqueSystemId::new().unwrap().value()).as_bytes(),
            )
            .unwrap(),
        );

        FilePath::new(path.as_bytes()).unwrap()
    }

    fn create_event<S: Service>(node: &Node<S>) -> (Listener<S>, Notifier<S>) {
        let service_name = generate_name();
        let service = node
            .service_builder(&service_name)
            .event()
            .open_or_create()
            .unwrap();
        (
            service.listener_builder().create().unwrap(),
            service.notifier_builder().create().unwrap(),
        )
    }

    fn create_socket() -> (UnixDatagramReceiver, UnixDatagramSender) {
        let uds_name = generate_uds_name();

        let receiver = UnixDatagramReceiverBuilder::new(&uds_name)
            .create()
            .unwrap();

        let sender = UnixDatagramSenderBuilder::new(&uds_name).create().unwrap();

        (receiver, sender)
    }

    #[test]
    fn attach_multiple_notifications_works<S: Service>()
    where
        <S::Event as Event>::Listener: SynchronousMultiplexing,
    {
        const LISTENER_LIMIT: usize = 16;
        const EXTERNAL_LIMIT: usize = 16;

        let node = NodeBuilder::new().create::<S>().unwrap();
        let sut = WaitSetBuilder::new().create::<S>().unwrap();
        let mut listeners = vec![];
        let mut sockets = vec![];
        let mut guards = vec![];

        for _ in 0..LISTENER_LIMIT {
            let (listener, _) = create_event::<S>(&node);
            listeners.push(listener);
        }

        for _ in 0..EXTERNAL_LIMIT {
            let (receiver, _) = create_socket();

            sockets.push(receiver);
        }

        assert_that!(sut.is_empty(), eq true);
        for (n, listener) in listeners.iter().enumerate() {
            assert_that!(sut.len(), eq n);
            guards.push(sut.attach_notification(listener).unwrap());
            assert_that!(sut.len(), eq n + 1);
            assert_that!(sut.is_empty(), eq false);
        }

        for (n, socket) in sockets.iter().enumerate() {
            assert_that!(sut.len(), eq n + listeners.len());
            guards.push(sut.attach_notification(socket).unwrap());
            assert_that!(sut.len(), eq n + 1 + listeners.len());
        }

        guards.clear();
        assert_that!(sut.is_empty(), eq true);
        assert_that!(sut.len(), eq 0);
    }

    #[test]
    fn attaching_same_notification_twice_fails<S: Service>()
    where
        <S::Event as Event>::Listener: SynchronousMultiplexing,
    {
        let node = NodeBuilder::new().create::<S>().unwrap();
        let sut = WaitSetBuilder::new().create::<S>().unwrap();

        let (listener, _) = create_event::<S>(&node);
        let (receiver, _) = create_socket();

        let _guard = sut.attach_notification(&listener);
        assert_that!(sut.attach_notification(&listener).err(), eq Some(WaitSetAttachmentError::AlreadyAttached));

        let _guard = sut.attach_notification(&receiver);
        assert_that!(sut.attach_notification(&receiver).err(), eq Some(WaitSetAttachmentError::AlreadyAttached));
    }

    #[test]
    fn attaching_same_deadline_twice_fails<S: Service>()
    where
        <S::Event as Event>::Listener: SynchronousMultiplexing,
    {
        let node = NodeBuilder::new().create::<S>().unwrap();
        let sut = WaitSetBuilder::new().create::<S>().unwrap();

        let (listener, _) = create_event::<S>(&node);
        let (receiver, _) = create_socket();

        let _guard = sut.attach_deadline(&listener, TIMEOUT);
        assert_that!(sut.attach_deadline(&listener, TIMEOUT).err(), eq Some(WaitSetAttachmentError::AlreadyAttached));

        let _guard = sut.attach_deadline(&receiver, TIMEOUT);
        assert_that!(sut.attach_deadline(&receiver, TIMEOUT).err(), eq Some(WaitSetAttachmentError::AlreadyAttached));
    }

    #[test]
    fn run_lists_all_notifications<S: Service>()
    where
        <S::Event as Event>::Listener: SynchronousMultiplexing,
    {
        let node = NodeBuilder::new().create::<S>().unwrap();
        let sut = WaitSetBuilder::new().create::<S>().unwrap();

        let (listener_1, notifier_1) = create_event::<S>(&node);
        let (listener_2, _notifier_2) = create_event::<S>(&node);
        let (receiver_1, sender_1) = create_socket();
        let (receiver_2, _sender_2) = create_socket();

        let listener_1_guard = sut.attach_notification(&listener_1).unwrap();
        let listener_2_guard = sut.attach_notification(&listener_2).unwrap();
        let receiver_1_guard = sut.attach_notification(&receiver_1).unwrap();
        let receiver_2_guard = sut.attach_notification(&receiver_2).unwrap();

        notifier_1.notify().unwrap();
        sender_1.try_send(b"bla").unwrap();

        let mut listener_1_triggered = false;
        let mut listener_2_triggered = false;
        let mut receiver_1_triggered = false;
        let mut receiver_2_triggered = false;

        let wait_event = sut
            .run(|attachment_id| {
                if attachment_id.event_from(&listener_1_guard) {
                    listener_1_triggered = true;
                } else if attachment_id.event_from(&listener_2_guard) {
                    listener_2_triggered = true;
                } else if attachment_id.event_from(&receiver_1_guard) {
                    receiver_1_triggered = true;
                } else if attachment_id.event_from(&receiver_2_guard) {
                    receiver_2_triggered = true;
                } else {
                    test_fail!("only attachments shall trigger");
                }
            })
            .unwrap();

        assert_that!(wait_event, eq WaitEvent::Notification);

        assert_that!(listener_1_triggered, eq true);
        assert_that!(receiver_1_triggered, eq true);
    }

    #[test]
    fn run_with_tick_blocks_for_at_least_timeout<S: Service>()
    where
        <S::Event as Event>::Listener: SynchronousMultiplexing,
    {
        let _watchdog = Watchdog::new();
        let node = NodeBuilder::new().create::<S>().unwrap();
        let sut = WaitSetBuilder::new().create::<S>().unwrap();

        let (listener, _) = create_event::<S>(&node);
        let _guard = sut.attach_notification(&listener);
        let tick_guard = sut.attach_tick(TIMEOUT).unwrap();

        let start = Instant::now();
        let wait_event = sut
            .run(|id| {
                assert_that!(id.event_from(&tick_guard), eq true);
            })
            .unwrap();

        assert_that!(wait_event, eq WaitEvent::Tick);
        assert_that!(start.elapsed(), time_at_least TIMEOUT);
    }

    // #[test]
    // fn blocking_wait_blocks<S: Service + 'static>()
    // where
    //     <S::Event as Event>::Listener: SynchronousMultiplexing,
    // {
    //     let _watchdog = Watchdog::new();
    //     let node = NodeBuilder::new().create::<S>().unwrap();
    //     let sut = WaitSetBuilder::new().create::<S>().unwrap();

    //     let service_name = generate_name();
    //     let service = node
    //         .service_builder(&service_name)
    //         .event()
    //         .open_or_create()
    //         .unwrap();

    //     let listener = service.listener_builder().create().unwrap();
    //     let _guard = sut.attach(&listener);

    //     let start = Instant::now();
    //     let barrier = Arc::new(Barrier::new(2));
    //     let barrier_thread = barrier.clone();

    //     let t1 = std::thread::spawn(move || {
    //         let notifier = service.notifier_builder().create().unwrap();
    //         barrier_thread.wait();
    //         std::thread::sleep(TIMEOUT);
    //         notifier.notify().unwrap();
    //     });

    //     barrier.wait();
    //     let wait_event = sut.blocking_wait(|_| {}).unwrap();

    //     assert_that!(wait_event, eq WaitEvent::Notification);
    //     assert_that!(start.elapsed(), time_at_least TIMEOUT);

    //     t1.join().unwrap();
    // }

    #[instantiate_tests(<iceoryx2::service::ipc::Service>)]
    mod ipc {}

    // #[instantiate_tests(<iceoryx2::service::local::Service>)]
    // mod local {}
}
