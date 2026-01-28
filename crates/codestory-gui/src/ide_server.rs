use codestory_core::protocol::IdeMessage;
use codestory_events::{Event, EventBus};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct IdeServer {
    event_bus: EventBus,
    pub port: u16,
}

impl IdeServer {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            event_bus,
            port: 6667,
        }
    }

    pub fn start(&self) {
        let event_bus = self.event_bus.clone();
        let port = self.port;

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let addr = format!("127.0.0.1:{}", port);
                let listener = match TcpListener::bind(&addr).await {
                    Ok(l) => {
                        tracing::info!("IDE Server listening on {}", addr);
                        l
                    }
                    Err(e) => {
                        tracing::error!("Failed to bind IDE Server on {}: {}", addr, e);
                        return;
                    }
                };

                loop {
                    match listener.accept().await {
                        Ok((mut socket, _)) => {
                            let event_bus = event_bus.clone();
                            tokio::spawn(async move {
                                let mut buf = [0; 1024];
                                loop {
                                    match socket.read(&mut buf).await {
                                        Ok(0) => return, // Connection closed
                                        Ok(n) => {
                                            if let Ok(msg_str) = std::str::from_utf8(&buf[0..n]) {
                                                // Handle concatenated messages if necessary, simpler for now
                                                for line in msg_str.lines() {
                                                    if let Ok(msg) = serde_json::from_str::<IdeMessage>(line) {
                                                        match msg {
                                                            IdeMessage::Ping { id } => {
                                                                let response = IdeMessage::Pong { id };
                                                                if let Ok(resp_str) = serde_json::to_string(&response) {
                                                                    let _ = socket.write_all(format!("{}\n", resp_str).as_bytes()).await;
                                                                }
                                                            }
                                                            IdeMessage::SetActiveLocation { file_path, line, column: _ } => {
                                                                event_bus.publish(Event::ScrollToLine {
                                                                    file: std::path::PathBuf::from(file_path),
                                                                    line: line as usize,
                                                                });
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Err(_) => return,
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!("Accept failed: {}", e);
                        }
                    }
                }
            });
        });
    }
}
