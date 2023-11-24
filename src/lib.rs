use airtable_flows::create_record;
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
use log;
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

pub async fn upload_airtable(question: &str, answer: &str) {
    let airtable_token_name = env::var("airtable_token_name").unwrap_or("github".to_string());
    let airtable_base_id = env::var("airtable_base_id").unwrap_or("appmhvMGsMRPmuUWJ".to_string());
    let airtable_table_name = env::var("airtable_table_name").unwrap_or("mention".to_string());

    let data = serde_json::json!({
        "Question": question,
        "Answer": answer,
    });
    match create_record(
        &airtable_token_name,
        &airtable_base_id,
        &airtable_table_name,
        data.clone(),
    ) {
        () => log::info!("Uploaded to airtable: {}", answer.to_string()),
    }
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

    let chunks = split_text_into_chunks(&user_input, 2000);
    let mut count = 0;

    for user_input in chunks {
        if let Ok(Some(qa_pairs)) = gen_pair(user_input).await {
            for (question, answer) in qa_pairs {
                count += 1;

                wtr.write_record(&[question, answer])
                    .expect("Failed to write record");
            }
        }
        log::error!("produced {} QAs", count);
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
    let bot_prompt = env::var("BOT_PROMPT").unwrap_or(
    "As a highly skilled language assistant, you are tasked with generating a scalable number of informative question and answer pairs from the provided text. The number of pairs generated should correspond to the length of the text: more pairs for longer texts, fewer pairs for shorter texts. Analyze the text at both the micro level—detailing specific segments—and the macro level—capturing overarching themes. Craft Q&A pairs that are relevant, accurate, and varied in type (factual, inferential, thematic). Your questions should be engaging, and answers should be concise, both reflecting the text's intent. Aim for a comprehensive dataset that is rich in content and suitable for training language models, balancing the depth and breadth of information without redundancy."
.into());

    let user_input = format!(
        "Here is the user input to work with: {}. Your task is to dissect this text for both granular details and broader themes, crafting multiple Q&A pairs for each part. The questions should cover different types: factual, inferential, thematic, etc. Answers must be concise and reflective of the text's intent. Please generate as many question and answers as possible. Provide the results in the following JSON format:
        {{
            \"qa_pairs\": [
                {{
                    \"question\": \"<Your question>\",
                    \"answer\": \"<Your answer>\"
                }},
                // ... additional Q&A pairs based on text length
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

    let chat = match client.chat().create(request).await {
        Ok(chat) => chat,

        Err(e) => {
            log::error!("Failed to create chat: {:?}", e);
            return Ok(None);
        }
    };

    #[derive(serde::Deserialize)]
    struct QaPair {
        question: String,
        answer: String,
    }

    let mut qa_pairs_vec = Vec::new();
    if let Some(qa_pairs_json) = &chat.choices[0].message.content {
        let deserialized: HashMap<String, Vec<QaPair>> = match serde_json::from_str(&qa_pairs_json)
        {
            Ok(deserialized) => deserialized,
            Err(e) => {
                log::error!("Failed to deserialize qa_pairs_json: {:?}", e);
                return Ok(None);
            }
        };

        if let Some(qa_pairs) = deserialized.get("qa_pairs") {
            qa_pairs_vec = qa_pairs
                .iter()
                .map(|qa| (qa.question.clone(), qa.answer.clone()))
                .collect();
        }
    }
    for (question, answer) in &qa_pairs_vec {
        upload_airtable(question, answer).await;
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
