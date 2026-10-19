//! TypeSafe's Jev: send a `state` and named questions, get every answer at
//! once with probabilities. It does not write text. See
//! <https://docs.typesafe.ai/api>.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AiError, excerpt, with_retries};

/// Pinned, so tuned thresholds keep meaning the same. The `jev-latest`
/// alias moves with new releases.
pub const DEFAULT_MODEL: &str = "jev-1.13.0";
const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const SERVICE: &str = "Jev";

/// One question. `instructions` may be text or an object that holds the
/// question in one field and data it refers to in others.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// Yes or no: the answer is the chance of yes.
    Noul {
        instructions: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// One option out of up to 255, each with a description or none.
    Choice { instructions: Value, criteria: BTreeMap<String, Option<String>> },
    /// A rating on 2 to 10 ordered levels.
    Score { instructions: Value, criteria: Vec<String> },
}

impl Question {
    pub fn yes_no(instructions: impl Into<Value>) -> Self {
        Self::Noul { instructions: instructions.into(), criteria: None }
    }
}

/// What a yes and a no mean.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub yes: String,
    #[serde(rename = "false")]
    pub no: String,
}

/// One answer, under the same name as its question.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    /// From 0 (no) to 1 (yes).
    Noul { noul: f64 },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    /// `score` is the level index, weighted by probability, so it can land
    /// between levels.
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

impl Answer {
    /// The chance of yes, for a yes/no answer.
    pub fn yes(&self) -> Option<f64> {
        match self {
            Self::Noul { noul } => Some(*noul),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct Reply {
    answers: BTreeMap<String, Answer>,
}

pub struct Jev {
    http: reqwest::Client,
    key: String,
    model: String,
    endpoint: String,
}

impl Jev {
    pub fn new(key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::with_endpoint(key, model, ENDPOINT)
    }

    pub fn with_endpoint(key: impl Into<String>, model: impl Into<String>, endpoint: impl Into<String>) -> Self {
        let model = model.into();
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            key: key.into(),
            model: if model.trim().is_empty() { DEFAULT_MODEL.to_owned() } else { model },
            endpoint: endpoint.into(),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Asks every question about `state` in one call.
    pub async fn ask(&self, state: &Value, questions: &BTreeMap<String, Question>) -> Result<BTreeMap<String, Answer>, AiError> {
        let body = serde_json::json!({ "state": state, "model": self.model, "questions": questions });
        let reply: Reply = with_retries(|| async {
            let response = self
                .http
                .post(&self.endpoint)
                .bearer_auth(&self.key)
                .json(&body)
                .send()
                .await
                .map_err(|err| AiError::network(SERVICE, err))?;
            let status = response.status();
            let text = response.text().await.map_err(|err| AiError::network(SERVICE, err))?;
            if !status.is_success() {
                return Err(AiError::Status { service: SERVICE, status: status.as_u16(), body: excerpt(&text) });
            }
            serde_json::from_str(&text).map_err(|err| AiError::answer(SERVICE, err.to_string()))
        })
        .await?;
        for name in questions.keys() {
            if !reply.answers.contains_key(name) {
                return Err(AiError::answer(SERVICE, format!("no answer for `{name}`")));
            }
        }
        Ok(reply.answers)
    }

    /// Checks the key and the model with one small question.
    pub async fn test(&self) -> Result<(), AiError> {
        let questions = BTreeMap::from([("ok".to_owned(), Question::yes_no("Is this a greeting?"))]);
        self.ask(&Value::String("Hello".into()), &questions).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::serve;

    #[test]
    fn questions_are_sent_as_the_api_wants() {
        let question = Question::Noul {
            instructions: "Is it urgent?".into(),
            criteria: Some(NoulCriteria { yes: "Time-sensitive".into(), no: "Not".into() }),
        };
        let json = serde_json::to_value(&question).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "noul", "instructions": "Is it urgent?", "criteria": { "true": "Time-sensitive", "false": "Not" } })
        );
        let choice = Question::Choice {
            instructions: "Which team?".into(),
            criteria: BTreeMap::from([("billing".into(), Some("Payments".into())), ("sales".into(), None)]),
        };
        assert_eq!(serde_json::to_value(&choice).unwrap()["criteria"]["sales"], Value::Null);
    }

    #[tokio::test]
    async fn asks_and_reads_every_answer() {
        let body = r#"{"model":"jev-1.13.0","answers":{
            "urgent":{"type":"noul","noul":0.95},
            "team":{"type":"choice","choice":"billing","probabilities":{"billing":0.88,"sales":0.12},"confidence":0.81},
            "mood":{"type":"score","score":1.05,"legend":{"0":"Calm","1":"Upset"},"probabilities":{"0":0.0,"1":0.95},"confidence":0.92}
        },"usage":{"input_tokens":3,"output_tokens":1}}"#;
        let (address, server) = serve(vec![(529, "{}".into()), (200, body.into())]).await;
        let jev = Jev::with_endpoint("test-key", "", format!("{address}/v1/systemone"));
        let questions = BTreeMap::from([
            ("urgent".to_owned(), Question::yes_no("Urgent?")),
            ("team".to_owned(), Question::Choice { instructions: "Team?".into(), criteria: BTreeMap::new() }),
            ("mood".to_owned(), Question::Score { instructions: "Mood?".into(), criteria: vec!["Calm".into(), "Upset".into()] }),
        ]);
        let answers = jev.ask(&Value::String("Payouts fail".into()), &questions).await.unwrap();
        assert_eq!(answers["urgent"].yes(), Some(0.95));
        assert!(matches!(&answers["team"], Answer::Choice { choice, .. } if choice == "billing"));
        assert!(matches!(&answers["mood"], Answer::Score { score, .. } if (*score - 1.05).abs() < 1e-9));
        let seen = server.await.unwrap();
        // Overloaded once, so it was asked again.
        assert_eq!(seen.len(), 2);
        assert!(seen[1].contains("authorization: Bearer test-key") || seen[1].contains("Authorization: Bearer test-key"));
        assert!(seen[1].contains(r#""model":"jev-1.13.0""#));
    }

    #[tokio::test]
    async fn a_bad_key_is_reported() {
        let (address, _server) = serve(vec![(401, r#"{"error":"invalid key"}"#.into())]).await;
        let jev = Jev::with_endpoint("bad", DEFAULT_MODEL, address);
        let err = jev.test().await.unwrap_err();
        assert!(matches!(err, AiError::Status { status: 401, .. }), "{err}");
    }
}
