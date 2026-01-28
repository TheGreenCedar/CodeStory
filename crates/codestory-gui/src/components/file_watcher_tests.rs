#[cfg(test)]
mod tests {
    use crate::components::file_watcher::FileWatcher;
    use codestory_events::{Event, EventBus};
    use std::fs::File;
    use std::io::Write;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    #[test]
    fn test_file_watcher_debouncing() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.cpp");

        let bus = EventBus::new();
        let mut watcher = FileWatcher::new(bus.clone()).unwrap();
        watcher.watch(dir.path()).unwrap();

        let rx = bus.receiver();

        // Write to file multiple times rapidly
        for i in 0..5 {
            let mut f = File::create(&file_path).unwrap();
            writeln!(f, "content {}", i).unwrap();
            f.sync_all().unwrap();
        }

        // Polling loop to process events
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(1500) {
            watcher.poll();
            std::thread::sleep(Duration::from_millis(50));
        }

        // Check if we received the event
        let mut event_count = 0;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, Event::FilesChanged { .. }) {
                event_count += 1;
            }
        }

        // Should be at least 1 due to debouncing
        assert!(event_count >= 1);
    }

    #[test]
    fn test_file_watcher_ignore_patterns() {
        let dir = tempdir().unwrap();
        let build_dir = dir.path().join("build");
        std::fs::create_dir(&build_dir).unwrap();
        let file_path = build_dir.join("ignored.o");

        let bus = EventBus::new();
        let mut watcher = FileWatcher::new(bus.clone()).unwrap();
        watcher.watch(dir.path()).unwrap();
        // watcher.add_ignore_pattern("build/**".to_string()); // Already ignored by default "build"

        let rx = bus.receiver();

        // Write to ignored file
        let mut f = File::create(&file_path).unwrap();
        writeln!(f, "content").unwrap();
        f.sync_all().unwrap();

        // Polling loop
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(1000) {
            watcher.poll();
            std::thread::sleep(Duration::from_millis(50));
        }

        // Should NOT have received any FilesChanged events for this file
        let mut event_received = false;
        while let Ok(event) = rx.try_recv() {
            if let Event::FilesChanged { paths } = event
                && paths
                    .iter()
                    .any(|p| p.to_string_lossy().contains("ignored.o"))
            {
                event_received = true;
            }
        }

        assert!(!event_received);
    }
}
