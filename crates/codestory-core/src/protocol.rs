use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IdeMessage {
    Ping {
        id: u64,
    },
    Pong {
        id: u64,
    },

    // IDE -> CodeStory
    SetActiveLocation {
        file_path: String,
        line: u32,
        column: u32,
    },

    // CodeStory -> IDE
    OpenLocation {
        file_path: String,
        line: u32,
        column: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialization() {
        let msg = IdeMessage::Ping { id: 123 };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"Ping","id":123}"#);

        let msg = IdeMessage::SetActiveLocation {
            file_path: "src/main.rs".to_string(),
            line: 10,
            column: 5,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"SetActiveLocation","file_path":"src/main.rs","line":10,"column":5}"#
        );
    }

    #[test]
    fn test_deserialization() {
        let json = r#"{"type":"Pong","id":456}"#;
        let msg: IdeMessage = serde_json::from_str(json).unwrap();
        match msg {
            IdeMessage::Pong { id } => assert_eq!(id, 456),
            _ => panic!("Wrong message type"),
        }
    }
}
