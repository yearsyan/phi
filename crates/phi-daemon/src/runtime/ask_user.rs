use std::collections::HashSet;

use async_trait::async_trait;
use phi::{Tool, ToolDefinition, ToolEffect, ToolError, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use super::AskUserId;

pub const ASK_USER_TOOL_NAME: &str = "askuser";

const ASK_USER_DESCRIPTION: &str = r#"Use this tool only when you are blocked on a decision that is genuinely the user's to make: one you cannot resolve from the request, the code, or sensible defaults.

Usage notes:
- Users will always be able to select "Other" to provide custom text input
- Use multiSelect: true to allow multiple answers to be selected for a question
- If you recommend a specific option, make that the first option in the list and add "(Recommended)" at the end of the label
- Do not use this tool merely to ask whether to proceed or whether a plan is ready

Reserve this for decisions where the user's answer changes what you do next — not for choices with a conventional default or facts you can verify in the codebase yourself. In those cases pick the obvious option, mention it in your response, and proceed.

Preview feature:
Use the optional preview field on options when presenting concrete artifacts that users need to visually compare:
- ASCII mockups of UI layouts or components
- Code snippets showing different implementations
- Diagram variations
- Configuration examples

Preview content is rendered as markdown in a monospace box. Multi-line text with newlines is supported. When any option has a preview, the UI switches to a side-by-side layout with a vertical option list on the left and preview on the right. Do not use previews for simple preference questions where labels and descriptions suffice. Previews are only supported for single-select questions."#;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskUserOption {
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskUserQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<AskUserOption>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AskUserRequest {
    pub ask_id: AskUserId,
    pub questions: Vec<AskUserQuestion>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AskUserAnswer {
    pub question_index: usize,
    #[serde(default)]
    pub selected_options: Vec<String>,
    #[serde(default)]
    pub custom_text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AskUserArguments {
    questions: Vec<AskUserQuestion>,
}

pub(super) struct PendingAskUserRequest {
    pub request: AskUserRequest,
    pub reply: oneshot::Sender<Result<Vec<AskUserAnswer>, String>>,
}

#[derive(Clone)]
pub(super) struct AskUserTool {
    requests: mpsc::UnboundedSender<PendingAskUserRequest>,
}

impl AskUserTool {
    pub(super) fn channel() -> (Self, mpsc::UnboundedReceiver<PendingAskUserRequest>) {
        let (requests, receiver) = mpsc::unbounded_channel();
        (Self { requests }, receiver)
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            ASK_USER_TOOL_NAME,
            ASK_USER_DESCRIPTION,
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 3,
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "question": { "type": "string", "minLength": 1 },
                                "header": { "type": "string", "minLength": 1, "maxLength": 12 },
                                "options": {
                                    "type": "array",
                                    "minItems": 2,
                                    "maxItems": 4,
                                    "items": {
                                        "type": "object",
                                        "additionalProperties": false,
                                        "properties": {
                                            "label": { "type": "string", "minLength": 1 },
                                            "description": { "type": "string", "minLength": 1 },
                                            "preview": { "type": "string", "minLength": 1 }
                                        },
                                        "required": ["label", "description"]
                                    }
                                },
                                "multiSelect": { "type": "boolean", "default": false }
                            },
                            "required": ["question", "header", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::Internal
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let arguments: AskUserArguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::new(format!("invalid askuser arguments: {error}")))?;
        validate_questions(&arguments.questions).map_err(ToolError::new)?;

        let request = AskUserRequest {
            ask_id: AskUserId::new(),
            questions: arguments.questions,
        };
        let (reply, response) = oneshot::channel();
        self.requests
            .send(PendingAskUserRequest {
                request: request.clone(),
                reply,
            })
            .map_err(|_| ToolError::new("askuser runtime is unavailable"))?;
        let answers = response
            .await
            .map_err(|_| ToolError::new("askuser request was cancelled"))?
            .map_err(ToolError::new)?;

        Ok(ToolOutput::success(format_answers(
            &request.questions,
            &answers,
        )?))
    }
}

fn validate_questions(questions: &[AskUserQuestion]) -> Result<(), String> {
    if !(1..=3).contains(&questions.len()) {
        return Err("askuser requires between 1 and 3 questions".to_owned());
    }
    for (index, question) in questions.iter().enumerate() {
        if question.question.trim().is_empty() {
            return Err(format!("question {index} must not be empty"));
        }
        let header_length = question.header.chars().count();
        if !(1..=12).contains(&header_length) {
            return Err(format!(
                "question {index} header must contain between 1 and 12 characters"
            ));
        }
        if !(2..=4).contains(&question.options.len()) {
            return Err(format!("question {index} requires between 2 and 4 options"));
        }
        let mut labels = HashSet::with_capacity(question.options.len());
        for option in &question.options {
            if option.label.trim().is_empty() {
                return Err(format!("question {index} has an empty option label"));
            }
            if option.description.trim().is_empty() {
                return Err(format!(
                    "question {index} option {:?} has an empty description",
                    option.label
                ));
            }
            if !labels.insert(option.label.as_str()) {
                return Err(format!(
                    "question {index} has duplicate option label {:?}",
                    option.label
                ));
            }
            if option
                .preview
                .as_ref()
                .is_some_and(|preview| preview.is_empty())
            {
                return Err(format!(
                    "question {index} option {:?} has an empty preview",
                    option.label
                ));
            }
            if question.multi_select && option.preview.is_some() {
                return Err(format!(
                    "question {index} cannot use previews when multiSelect is true"
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_answers(
    questions: &[AskUserQuestion],
    answers: &[AskUserAnswer],
) -> Result<(), String> {
    if answers.len() != questions.len() {
        return Err(format!(
            "expected {} answers, received {}",
            questions.len(),
            answers.len()
        ));
    }
    let mut answered = HashSet::with_capacity(answers.len());
    for answer in answers {
        let Some(question) = questions.get(answer.question_index) else {
            return Err(format!(
                "question_index {} is out of range",
                answer.question_index
            ));
        };
        if !answered.insert(answer.question_index) {
            return Err(format!(
                "question_index {} was answered more than once",
                answer.question_index
            ));
        }
        let mut selected = HashSet::with_capacity(answer.selected_options.len());
        for label in &answer.selected_options {
            if !selected.insert(label.as_str()) {
                return Err(format!(
                    "question_index {} selected option {:?} more than once",
                    answer.question_index, label
                ));
            }
            if !question.options.iter().any(|option| option.label == *label) {
                return Err(format!(
                    "question_index {} selected unknown option {:?}",
                    answer.question_index, label
                ));
            }
        }
        let has_custom_text = match &answer.custom_text {
            Some(custom_text) if custom_text.trim().is_empty() => {
                return Err(format!(
                    "question_index {} custom_text must not be empty",
                    answer.question_index
                ));
            }
            Some(_) => true,
            None => false,
        };
        let selection_count = answer.selected_options.len() + usize::from(has_custom_text);
        if selection_count == 0 {
            return Err(format!(
                "question_index {} requires an option or custom_text",
                answer.question_index
            ));
        }
        if !question.multi_select && selection_count != 1 {
            return Err(format!(
                "question_index {} is single-select and requires exactly one answer",
                answer.question_index
            ));
        }
    }
    Ok(())
}

fn format_answers(
    questions: &[AskUserQuestion],
    answers: &[AskUserAnswer],
) -> Result<String, ToolError> {
    #[derive(Serialize)]
    struct ResultAnswer<'a> {
        question: &'a str,
        selected_options: &'a [String],
        custom_text: Option<&'a str>,
    }

    #[derive(Serialize)]
    struct ResultEnvelope<'a> {
        answers: Vec<ResultAnswer<'a>>,
    }

    let answers = questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let answer = answers
                .iter()
                .find(|answer| answer.question_index == index)
                .expect("validated askuser answers must cover every question");
            ResultAnswer {
                question: &question.question,
                selected_options: &answer.selected_options,
                custom_text: answer.custom_text.as_deref(),
            }
        })
        .collect();
    serde_json::to_string(&ResultEnvelope { answers })
        .map_err(|error| ToolError::new(format!("could not serialize askuser answers: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(multi_select: bool) -> AskUserQuestion {
        AskUserQuestion {
            question: "Which approach?".to_owned(),
            header: "Approach".to_owned(),
            options: vec![
                AskUserOption {
                    label: "A".to_owned(),
                    description: "Use A".to_owned(),
                    preview: None,
                },
                AskUserOption {
                    label: "B".to_owned(),
                    description: "Use B".to_owned(),
                    preview: None,
                },
            ],
            multi_select,
        }
    }

    #[test]
    fn accepts_a_custom_other_answer() {
        let questions = [question(false)];
        let answers = [AskUserAnswer {
            question_index: 0,
            selected_options: Vec::new(),
            custom_text: Some("My own choice".to_owned()),
        }];

        validate_answers(&questions, &answers).unwrap();
    }

    #[test]
    fn rejects_multiple_values_for_a_single_select_question() {
        let questions = [question(false)];
        let answers = [AskUserAnswer {
            question_index: 0,
            selected_options: vec!["A".to_owned(), "B".to_owned()],
            custom_text: None,
        }];

        assert!(validate_answers(&questions, &answers).is_err());
    }
}
