use crate::tools::{ToolDefinition, ToolHandler, substitute};
use std::collections::HashMap;

/// Execute a tool call with the given arguments.
/// Returns the result as a plain string to feed back to the model.
pub async fn execute(
    tool: &ToolDefinition,
    args: &serde_json::Value,
    http: &reqwest::Client,
) -> anyhow::Result<String> {
    match &tool.handler {
        ToolHandler::Http { method, url, headers, body } => {
            execute_http(method, url, headers, body.as_deref(), args, http).await
        }
        ToolHandler::Mqtt { broker, command_topic, payload, response_topic, timeout_ms } => {
            execute_mqtt(broker, command_topic, payload, response_topic.as_deref(), *timeout_ms, args).await
        }
        ToolHandler::Builtin { name } => execute_builtin(name),
    }
}

async fn execute_http(
    method: &str,
    url_template: &str,
    headers: &HashMap<String, String>,
    body_template: Option<&str>,
    args: &serde_json::Value,
    http: &reqwest::Client,
) -> anyhow::Result<String> {
    let url = substitute(url_template, args);

    let mut req = http.request(method.parse()?, &url);

    for (k, v) in headers {
        req = req.header(substitute(k, args), substitute(v, args));
    }

    if let Some(tmpl) = body_template {
        let body = substitute(tmpl, args);
        req = req.header("content-type", "application/json").body(body);
    }

    let resp = req.send().await?;
    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        anyhow::bail!("HTTP {status}: {text}");
    }

    Ok(text)
}

async fn execute_mqtt(
    broker: &str,
    command_topic_tmpl: &str,
    payload_tmpl: &str,
    response_topic_tmpl: Option<&str>,
    timeout_ms: u64,
    args: &serde_json::Value,
) -> anyhow::Result<String> {
    let command_topic = substitute(command_topic_tmpl, args);
    let payload = substitute(payload_tmpl, args);

    let (host, port) = parse_broker(broker)?;

    let client_id = format!("lite-llm-{}", uuid::Uuid::new_v4());
    let mut mqttoptions = rumqttc::MqttOptions::new(&client_id, host, port);
    mqttoptions.set_keep_alive(std::time::Duration::from_secs(10));

    let (client, mut eventloop) = rumqttc::AsyncClient::new(mqttoptions, 16);

    // Subscribe to response topic before publishing if we expect a response.
    let response_topic = response_topic_tmpl.map(|t| substitute(t, args));
    if let Some(ref topic) = response_topic {
        client.subscribe(topic, rumqttc::QoS::AtLeastOnce).await?;
    }

    client
        .publish(&command_topic, rumqttc::QoS::AtLeastOnce, false, payload)
        .await?;

    if response_topic.is_none() {
        // Fire-and-forget: drain until publish ack then return.
        let timeout = tokio::time::Duration::from_millis(timeout_ms);
        let _ = tokio::time::timeout(timeout, async {
            loop {
                if let Ok(rumqttc::Event::Outgoing(rumqttc::Outgoing::Publish(_))) =
                    eventloop.poll().await
                {
                    break;
                }
            }
        })
        .await;
        return Ok("ok".into());
    }

    // Wait for a message on the response topic.
    let timeout = tokio::time::Duration::from_millis(timeout_ms);
    let result = tokio::time::timeout(timeout, async {
        loop {
            match eventloop.poll().await {
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(p))) => {
                    return Ok(String::from_utf8_lossy(&p.payload).into_owned());
                }
                Err(e) => return Err(anyhow::anyhow!("MQTT error: {e}")),
                _ => {}
            }
        }
    })
    .await;

    match result {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(e),
        Err(_) => anyhow::bail!("MQTT response timed out after {timeout_ms}ms"),
    }
}

fn execute_builtin(name: &str) -> anyhow::Result<String> {
    match name {
        "datetime" => {
            let now = chrono::Local::now();
            Ok(format!(
                "{}, {} {}",
                now.format("%A, %B %-d, %Y"),
                now.format("%H:%M"),
                now.format("%Z"),
            ))
        }
        _ => anyhow::bail!("Unknown builtin tool: {name}"),
    }
}

fn parse_broker(broker: &str) -> anyhow::Result<(&str, u16)> {
    let mut parts = broker.rsplitn(2, ':');
    let port: u16 = parts
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("Invalid broker address: {broker}"))?;
    let host = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Invalid broker address: {broker}"))?;
    Ok((host, port))
}
