//! Minimal typed client for Jev via OpenRouter's decisions endpoint.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Map, Value, json};

const ENDPOINT: &str = "https://openrouter.ai/api/alpha/decisions";
const MODEL: &str = "~typesafe/jev-latest";

/// A question for Jev. Jev evaluates each one independently against the same state.
#[derive(Debug, Clone)]
pub enum Question {
    /// Binary judgement -> probability of `true`.
    Noul { instructions: String, when_true: String, when_false: String },
    /// Pick one of these (key, description) options.
    Choice { instructions: String, options: Vec<(String, String)> },
    /// Ordered levels, lowest first.
    Score { instructions: String, levels: Vec<String> },
}

impl Question {
    pub fn to_json(&self) -> Value {
        match self {
            Question::Noul { instructions, when_true, when_false } => json!({
                "type": "noul",
                "instructions": instructions,
                "criteria": { "true": when_true, "false": when_false },
            }),
            Question::Choice { instructions, options } => {
                let criteria: Map<String, Value> =
                    options.iter().map(|(k, d)| (k.clone(), Value::String(d.clone()))).collect();
                json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
            }
            Question::Score { instructions, levels } => json!({
                "type": "score",
                "instructions": instructions,
                "criteria": levels,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Answers {
    pub raw: Value,
    pub latency: Duration,
}

impl Answers {
    fn get(&self, key: &str) -> Option<&Value> {
        self.raw.get("answers")?.get(key)
    }

    /// Probability (0..1) that the noul question is true.
    pub fn noul(&self, key: &str) -> Option<f64> {
        let a = self.get(key)?;
        a.get("noul").and_then(Value::as_f64).or_else(|| a.as_f64())
    }

    /// Winning option key and its confidence (if provided).
    pub fn choice(&self, key: &str) -> Option<(String, Option<f64>)> {
        let a = self.get(key)?;
        let c = a.get("choice")?.as_str()?.to_string();
        Some((c, a.get("confidence").and_then(Value::as_f64)))
    }

    /// Per-option probabilities for a choice/score question.
    pub fn probabilities(&self, key: &str) -> Vec<(String, f64)> {
        let mut v = Vec::new();
        match self.get(key).and_then(|a| a.get("probabilities")) {
            Some(Value::Object(m)) => {
                for (k, p) in m {
                    if let Some(p) = p.as_f64() {
                        v.push((k.clone(), p));
                    }
                }
            }
            Some(Value::Array(a)) => {
                for (i, p) in a.iter().enumerate() {
                    if let Some(p) = p.as_f64() {
                        v.push((i.to_string(), p));
                    }
                }
            }
            _ => {}
        }
        v
    }

    /// Continuous score (0-based level index; may fall between levels).
    pub fn score(&self, key: &str) -> Option<f64> {
        let a = self.get(key)?;
        a.get("score").and_then(Value::as_f64).or_else(|| a.as_f64())
    }
}

pub struct Jev {
    http: reqwest::blocking::Client,
    api_key: String,
}

impl Jev {
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .context("OPENROUTER_API_KEY not set (put it in .env)")?;
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { http, api_key })
    }

    pub fn ask<K: AsRef<str>>(&self, state: &Value, questions: &[(K, Question)]) -> Result<Answers> {
        let qs: Map<String, Value> =
            questions.iter().map(|(k, q)| (k.as_ref().to_string(), q.to_json())).collect();
        let body = Body { model: MODEL, state, questions: Value::Object(qs) };
        let start = Instant::now();
        let resp = self
            .http
            .post(ENDPOINT)
            .bearer_auth(&self.api_key)
            .header("X-OpenRouter-Title", "jamsesh")
            .json(&body)
            .send()
            .context("Jev request failed")?;
        let status = resp.status();
        let text = resp.text()?;
        let latency = start.elapsed();
        if !status.is_success() {
            bail!("Jev returned {status}: {text}");
        }
        let raw: Value = serde_json::from_str(&text).context("Jev response was not JSON")?;
        Ok(Answers { raw, latency })
    }
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    state: &'a Value,
    questions: Value,
}
