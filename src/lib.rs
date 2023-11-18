use async_openai::{
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
        CreateMessageRequestArgs, CreateRunRequestArgs, CreateThreadRequestArgs, MessageContent,
        RunStatus,
    },
    Client,
};
use dotenv::dotenv;
use flowsnet_platform_sdk::logger;
use lazy_static::lazy_static;
use once_cell::sync::Lazy;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use tokio::sync::Mutex;
use webhook_flows::{request_received, send_response};

static MESSAGES: Lazy<Mutex<Vec<ChatCompletionRequestMessage>>> = Lazy::new(|| {
    let mut messages = Vec::new();
    messages.push(
        ChatCompletionRequestSystemMessageArgs::default()
            .content("Perform function requests for the user")
            .build()
            .expect("Failed to build system message")
            .into(),
    );
    Mutex::new(messages)
});

#[no_mangle]
#[tokio::main(flavor = "current_thread")]
pub async fn run() {
    dotenv::dotenv().ok();
    logger::init();
    request_received(handler).await;
}

async fn handler(_headers: Vec<(String, String)>, _qry: HashMap<String, Value>, _body: Vec<u8>) {
    let body_str = match String::from_utf8(_body) {
        Ok(body) => body,
        Err(e) => {
            eprintln!("Failed to parse request body as UTF-8 string: {}", e);
            return;
        }
    };

    let raw_input: JsonResult<Value> = serde_json::from_str(&body_str);

    let question = match raw_input {
        Ok(json) => json["question"].to_string(),
        Err(e) => {
            eprintln!("Failed to parse request body as JSON: {}", e);
            "".to_string()
        }
    };

    // let user_login = _qry
    //     .get("login")
    //     .unwrap_or(&Value::Null)
    //     .as_str()
    //     .map(|n| n.to_string());    let OPENAI_API_KEY = std::env::var("OPENAI_API_KEY").unwrap();
    let mut messages = MESSAGES.lock().await.clone();

    let answer = match gen_pair(question, &mut messages).await {
        Ok(Some(response)) => response,
        _ => "Error processing response from OpenAI API".to_string(),
    };

    let formatted_answer = format!(
        r#"Question,Answer
"{question}","{answer}""#
    );

    send_response(
        200,
        vec![(
            String::from("content-type"),
            String::from("text/plain; charset=UTF-8"),
        )],
        formatted_answer.as_bytes().to_vec(),
    );
}

pub async fn gen_pair(
    user_input: String,
    messages: &mut Vec<ChatCompletionRequestMessage>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let client = Client::new();
    let user_msg_obj = ChatCompletionRequestUserMessageArgs::default()
        .content(user_input)
        .build()?
        .into();

    messages.push(user_msg_obj);

    let request = CreateChatCompletionRequestArgs::default()
        .max_tokens(512u16)
        .model("gpt-3.5-turbo-1106")
        .messages(messages.clone())
        .build()?;

    let chat = client.chat().create(request).await?;

    // let check = chat.choices.get(0).clone().unwrap();
    // send_message_to_channel("ik8", "general", format!("{:?}", check)).await;

    match chat.choices[0].message.clone().content {
        Some(res) => Ok(Some(res)),
        None => Ok(None),
    }
}
