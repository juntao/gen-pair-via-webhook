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
    // let bot_prompt = env::var("BOT_PROMPT").unwrap_or("You are a highly skilled language assistant with expertise in generating insightful question and answer pairs from provided texts. Your task is to carefully analyze the user's input text in two ways: first, by focusing on the details and nuances within small segments of the text, i.e. several adjacent lines pertaining to a common subject, and second, by considering the broader themes and ideas that emerge from the text as a whole.
    // For each segment of text, create multiple question and answer pairs that are not only relevant and accurate but also varied in type (e.g., factual, inferential, thematic). Ensure that the questions are clear, engaging, and capable of prompting deep understanding of the content. Likewise, the answers should be precise, informative, and succinct, reliably reflecting the information and intent of the original text.
    // Strive to produce a rich and comprehensive set of question and answer pairs that can serve as an effective training dataset for another language model. This dataset should help the model to learn the intricacies of text comprehension, ranging from specific details to overarching concepts. Quantity is important, but never at the expense of quality. Avoid redundancy and aim for a balance between breadth and depth in your questions and answers.".into());
    let bot_prompt = env::var("BOT_PROMPT").unwrap_or("As a highly skilled language assistant, your role is to generate informative question and answer pairs from a provided text. You must thoroughly analyze the text both at the micro level—focusing on specific details within small, related segments—and at the macro level, considering the overarching themes and concepts. Produce a diverse set of Q&A pairs that are relevant, varied, and accurate. Your questions should be clear and engaging, promoting a deep understanding of the content, while the answers should be precise and informative. The goal is to create a rich and comprehensive dataset that balances quantity with quality, serving as a valuable resource for training other language models. Please avoid redundancy and strive for a balance in your output.".into());

    let user_input = format!(
        "Here is the user input to work with: {}. Your task is to dissect this text for both granular details and broader themes, crafting multiple Q&A pairs for each part. The questions should cover different types: factual, inferential, thematic, etc. Answers must be concise and reflective of the text's intent. Please generate as many question and answers as possible. Provide the results in the following JSON format:
        {{
            \"qa_pairs\": [
                {{
                    \"question\": \"<Your question>\",
                    \"answer\": \"<Your answer>\"
                }},
                // ... additional Q&A pairs
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
