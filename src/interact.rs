use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
pub struct Interact {
    #[serde(rename = "type")]
    pub interaction_type: u8,
    pub application_id: String,
    pub channel_id: String,
    pub session_id: String,
    pub data: InteractionData,
    pub nonce: String,
    pub analytics_location: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InteractionData {
    pub version: String,
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub command_type: u8,
    pub options: Vec<InteractionOption>,
    pub application_command: ApplicationCommand,
    pub attachments: Vec<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InteractionOption {
    #[serde(rename = "type")]
    pub option_type: u8,
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApplicationCommand {
    pub id: String,
    #[serde(rename = "type")]
    pub command_type: u8,
    pub application_id: String,
    pub version: String,
    pub name: String,
    pub description: String,
    pub options: Vec<CommandOption>,
    pub dm_permission: bool,
    pub integration_types: Vec<u8>,
    pub global_popularity_rank: u64,
    pub description_localized: String,
    pub name_localized: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommandOption {
    #[serde(rename = "type")]
    pub option_type: u8,
    pub name: String,
    pub description: String,
    pub required: bool,
    pub choices: Vec<CommandOptionChoice>,
    pub description_localized: String,
    pub name_localized: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommandOptionChoice {
    pub name: String,
    pub value: String,
    pub name_localized: String,
}