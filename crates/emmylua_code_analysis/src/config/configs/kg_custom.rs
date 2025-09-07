use schemars::JsonSchema;
use serde_with::serde_derive::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KgCustom{
    #[serde(default = "default_false")]
    pub default_kg_require: bool,
    #[serde(default)]
    pub force_raw_require_re: Vec<String>,
}

impl Default for KgCustom{
    fn default() -> Self {
        Self {
            default_kg_require: default_false(),
            force_raw_require_re: vec![],
        }
    }
}

fn default_false() -> bool { false }
