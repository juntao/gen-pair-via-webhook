use async_openai::{
    types::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        ChatCompletionResponseFormat, ChatCompletionResponseFormatType,
        CreateChatCompletionRequestArgs,
    },
    Client,
};
use csv::{QuoteStyle, WriterBuilder};
use dotenv::dotenv;
use flowsnet_platform_sdk::logger;
use serde_json::Value;
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

    let mut wtr = WriterBuilder::new()
        .delimiter(b',')
        .quote_style(QuoteStyle::Always)
        .from_writer(vec![]);

    wtr.write_record(&["Question", "Answer"])
        .expect("Failed to header row record");

    let chunks = split_text_into_chunks(&user_input, 3000);

    for user_input in chunks {
        if let Ok(Some(qa_pairs)) = gen_pair(user_input).await {
            for (question, answer) in qa_pairs {
                wtr.write_record(&[question, answer])
                    .expect("Failed to write record");
            }
        }
    }

    let data = wtr.into_inner().expect("Failed to finalize CSV writing");
    let formatted_answer = String::from_utf8(data).expect("Failed to convert to String");

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
) -> Result<Option<Vec<(String, String)>>, Box<dyn std::error::Error>> {
    let bot_prompt = env::var("BOT_PROMPT").unwrap_or("You're a language expert. You are to generate question and answer pairs based on the user's input. Generate as many question and answer pairs as possible. The answers should be as concise as possible.".into());

    let user_input = format!(
        "Here is the user input to work with: {}, please generate question and answer pairs based on the input. Generate as many question and answer pairs as possible. The answers should be as concise as possible. Please provide result in JSON format like the following:
        {{
            \"qa_pairs\": [
                {{
                    \"question\": \"A question generated by the function\",
                    \"answer\": \"An answer generated by the function\"
                }}
            ]
        }}",
        user_input
    );

    let messages = vec![
        ChatCompletionRequestSystemMessageArgs::default()
            .content(&bot_prompt)
            .build()
            .expect("Failed to build system message")
            .into(),
        ChatCompletionRequestUserMessageArgs::default()
            .content(user_input)
            .build()?
            .into(),
    ];

    let client = Client::new();

    let response_format = ChatCompletionResponseFormat {
        r#type: ChatCompletionResponseFormatType::JsonObject,
    };

    let request = CreateChatCompletionRequestArgs::default()
        // .max_tokens(4096u16)
        .model("gpt-3.5-turbo-1106")
        .messages(messages)
        .response_format(response_format)
        .build()?;

    let chat = client.chat().create(request).await?;

    #[derive(serde::Deserialize)]
    struct QaPair {
        question: String,
        answer: String,
    }

    let mut qa_pairs_vec = Vec::new();
    if let Some(qa_pairs_json) = &chat.choices[0].message.content {
        let deserialized: HashMap<String, Vec<QaPair>> = serde_json::from_str(&qa_pairs_json)?;

        if let Some(qa_pairs) = deserialized.get("qa_pairs") {
            qa_pairs_vec = qa_pairs
                .iter()
                .map(|qa| (qa.question.clone(), qa.answer.clone()))
                .collect();
        }
    }

    Ok(Some(qa_pairs_vec))
}

pub fn split_text_into_chunks(raw_text: &str, max_words_per_chunk: usize) -> Vec<String> {
    let mut res = Vec::new();

    let mut sentences = String::new();
    let mut running_len = 0;
    for sen in raw_text.split("\n") {
        let len = sen.split_ascii_whitespace().count();
        running_len += len;
        if running_len > max_words_per_chunk {
            res.push(sentences.clone());
            sentences = sen.to_string();
            running_len = len;
        } else {
            sentences.push_str(&sen);
        }
    }

    res
}
