use async_openai::{
    types::{
        ChatCompletionFunctionsArgs, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        ChatCompletionTool, ChatCompletionToolArgs, ChatCompletionToolType,
        CreateChatCompletionRequestArgs, FinishReason,
    },
    Client,
};
use csv::{QuoteStyle, WriterBuilder};
use dotenv::dotenv;
use flowsnet_platform_sdk::logger;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use webhook_flows::{create_endpoint, request_handler, send_response};

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
    let bot_prompt = env::var("BOT_PROMPT").unwrap_or("You're a language expert. You are to generate question and answer pairs based on the user's input. Generate as many questions as possible. The answers should be as concise as possible.".into());

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

    let user_input = format!("Here is the user input to work with, please generate question and answer pairs based on the input. Generate as many questions as possible. The answers should be as concise as possible: {}", user_input);

    if let Ok(Some(qa_pairs)) = gen_pair(user_input, &mut messages).await {
        let mut wtr = WriterBuilder::new()
            .delimiter(b',')
            .quote_style(QuoteStyle::Always)
            .from_writer(vec![]);

        wtr.write_record(&["Question", "Answer"])
            .expect("Failed to header row record");

        for (question, answer) in qa_pairs {
            wtr.write_record(&[question, answer])
                .expect("Failed to write record");
        }

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
}

pub async fn gen_pair(
    user_input: String,
    messages: &mut Vec<ChatCompletionRequestMessage>,
) -> Result<Option<Vec<(String, String)>>, Box<dyn std::error::Error>> {
    let tools: Vec<ChatCompletionTool> = vec![ChatCompletionToolArgs::default()
    .r#type(ChatCompletionToolType::Function)
    .function(
        ChatCompletionFunctionsArgs::default()
            .name("genQa")
            .description("Generate a question and answer pair")
            .parameters(json!({
                "type": "object",
                "properties": {
                    "qa_pairs": {
                        "type": "array",
                        "items": {
                            "type": "array",
                            "minItems": 2,
                            "maxItems": 2,
                            "items": [
                                {
                                    "type": "string",
                                    "description": "The question part of the pair"
                                },
                                {
                                    "type": "string",
                                    "description": "The answer part of the pair"
                                }
                            ]
                        },
                        "description": "The array of question and answer pairs generated by the function"
                    },
                },
                "required": ["qa_pairs"],
            }))
            .build()
            .expect("Failed to build genQa function"),
    )
    .build()
    .expect("Failed to build genQa tool")];

    let client = Client::new();
    let user_msg_obj = ChatCompletionRequestUserMessageArgs::default()
        .content(user_input)
        .build()?
        .into();

    messages.push(user_msg_obj);

    let request = CreateChatCompletionRequestArgs::default()
        .max_tokens(2000u16)
        .model("gpt-3.5-turbo-1106")
        .messages(messages.clone())
        .tools(tools)
        .build()?;

    let chat = client.chat().create(request).await?;

    let check = chat.choices.get(0).clone().unwrap();

    log::info!("got response from chat: {:?}", check);

    let wants_to_use_function = chat
        .choices
        .get(0)
        .map(|choice| choice.finish_reason == Some(FinishReason::ToolCalls))
        .unwrap_or(false);

    let mut qa_pairs = Vec::new();
    if wants_to_use_function {
        let tool_calls = chat.choices[0].message.tool_calls.as_ref().unwrap();

        for tool_call in tool_calls {
            let function = &tool_call.function;

            match function.name.as_str() {
                "genQa" => {
                    let arguments_array =
                        serde_json::from_str::<Vec<Vec<String>>>(&function.arguments)?;
                    for argument_pair in arguments_array {
                        if argument_pair.len() == 2 {
                            let q = argument_pair[0].to_string();
                            let a = argument_pair[1].to_string();
                            qa_pairs.push((q, a));
                        }
                    }
                }
                _ => {}
            };
        }
    }

    Ok(Some(qa_pairs))
}
