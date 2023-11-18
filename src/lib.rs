use async_openai::{
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    },
    Client,
};
use dotenv::dotenv;
use flowsnet_platform_sdk::logger;
// use lazy_static::lazy_static;
// use once_cell::sync::Lazy;
// use tokio::sync::Mutex;
use csv::{QuoteStyle, WriterBuilder};
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use webhook_flows::{create_endpoint, request_handler, send_response};
// static MESSAGES: Lazy<Mutex<Vec<ChatCompletionRequestMessage>>> = Lazy::new(|| {
//     let mut messages = Vec::new();
//     messages.push(
//         ChatCompletionRequestSystemMessageArgs::default()
//             .content("Perform function requests for the user")
//             .build()
//             .expect("Failed to build system message")
//             .into(),
//     );
//     Mutex::new(messages)
// });

#[no_mangle]
#[tokio::main(flavor = "current_thread")]
pub async fn on_deploy() {
    dotenv().ok();
    logger::init();
    create_endpoint().await;
}

#[request_handler(GET, POST)]
async fn handler(
    _headers: Vec<(String, String)>,
    _subpath: String,
    _qry: HashMap<String, Value>,
    _body: Vec<u8>,
) {
    let bot_prompt = env::var("BOT_PROMPT").unwrap_or("You're a language expert. You are to generate a question and answer pair based on the user's input, please put the question and answer on two separate lines dilimited by \n".into());

    let mut messages = vec![ChatCompletionRequestSystemMessageArgs::default()
        .content(&bot_prompt)
        .build()
        .expect("Failed to build system message")
        .into()];

    let user_input = match String::from_utf8(_body) {
        Ok(body) => {
            log::info!("parsed body from request: {}", body.clone());
            body
        }
        Err(e) => {
            log::error!("Failed to parse request body as UTF-8 string: {}", e);
            return;
        }
    };

    // let user_login = _qry
    //     .get("login")
    //     .unwrap_or(&Value::Null)
    //     .as_str()
    //     .map(|n| n.to_string());    let OPENAI_API_KEY = std::env::var("OPENAI_API_KEY").unwrap();
    // let mut messages = MESSAGES.lock().await.clone();

    let response = match gen_pair(user_input, &mut messages).await {
        Ok(Some(response)) => {
            log::info!("Generated: {}", response.clone());
            response
        }
        Ok(None) => {
            log::error!("GPT failed to generate qa pair");
            return;
        }
        Err(_e) => {
            log::error!("gen_pair function failed: {}", _e);
            return;
        }
    };
    let (question, answer) = response.split_once('\n').unwrap_or(("", ""));

    let mut wtr = WriterBuilder::new()
        .quote_style(QuoteStyle::Always)
        .from_writer(vec![]);

    wtr.write_record(&[question, answer])
        .expect("Failed to write record");

    let formatted_answer =
        String::from_utf8(wtr.into_inner().expect("Failed to finalize CSV writing"))
            .expect("Failed to convert to String");

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
        .max_tokens(256u16)
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
