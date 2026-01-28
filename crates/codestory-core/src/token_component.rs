use crate::access::AccessKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenComponent {
    pub type_name: String,
    pub is_const: bool,
    pub is_static: bool,
    pub access: AccessKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub name: String,
    pub components: Vec<TokenComponent>,
}
