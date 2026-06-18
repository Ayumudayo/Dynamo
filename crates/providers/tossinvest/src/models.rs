use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TossInvestApiError {
    pub code: Option<String>,
    pub message: Option<String>,
}
