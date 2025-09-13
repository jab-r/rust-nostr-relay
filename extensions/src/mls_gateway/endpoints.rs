//! REST API endpoints for MLS Gateway mailbox services

use actix_web::{web, HttpResponse, Result as ActixResult};
use serde::{Deserialize, Serialize};
use serde_json::json;
use super::message_archive::MessageArchive;

#[derive(Debug, Serialize, Deserialize)]
pub struct MissedMessagesRequest {
    pub since: i64,  // Unix timestamp
    pub pubkey: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroupMessagesRequest {
    pub since: i64, // Unix timestamp
    pub group_id: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchivedMessage {
    pub id: String,
    pub kind: u32,
    pub content: String,
    pub tags: Vec<Vec<String>>,
    pub created_at: i64,
    pub pubkey: String,
    pub sig: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MissedMessagesResponse {
    pub messages: Vec<ArchivedMessage>,
    pub count: u32,
    pub has_more: bool,
}

/// Configure HTTP routes for MLS Gateway API
pub fn configure_routes(cfg: &mut web::ServiceConfig, prefix: &str) {
    cfg.service(
        web::scope(prefix)
            .route("/groups", web::get().to(list_groups))
            .route("/groups/{id}", web::get().to(get_group))
            .route("/keypackages", web::post().to(post_keypackage))
            .route("/keypackages", web::get().to(list_keypackages))
            .route("/keypackages/{id}/ack", web::post().to(ack_keypackage))
            .route("/welcome", web::post().to(post_welcome))
            .route("/welcome", web::get().to(list_welcomes))
            .route("/welcome/{id}/ack", web::post().to(ack_welcome))
            .route("/messages/missed", web::post().to(get_missed_messages))
            .route("/messages/group", web::post().to(get_group_messages)),
    );
}

/// List groups endpoint
async fn list_groups() -> ActixResult<HttpResponse> {
    Ok(HttpResponse::Ok().json(json!({
        "ok": true,
        "groups": []
    })))
}

/// Get group endpoint  
async fn get_group(path: web::Path<String>) -> ActixResult<HttpResponse> {
    let _group_id = path.into_inner();
    Ok(HttpResponse::Ok().json(json!({
        "ok": true,
        "group": null
    })))
}

/// Post key package endpoint
async fn post_keypackage() -> ActixResult<HttpResponse> {
    Ok(HttpResponse::Ok().json(json!({
        "ok": true,
        "id": "placeholder"
    })))
}

/// List key packages endpoint
async fn list_keypackages() -> ActixResult<HttpResponse> {
    Ok(HttpResponse::Ok().json(json!({
        "ok": true,
        "items": []
    })))
}

/// Acknowledge key package endpoint
async fn ack_keypackage(path: web::Path<String>) -> ActixResult<HttpResponse> {
    let _id = path.into_inner();
    Ok(HttpResponse::Ok().json(json!({
        "ok": true
    })))
}

/// Post welcome message endpoint
async fn post_welcome() -> ActixResult<HttpResponse> {
    Ok(HttpResponse::Ok().json(json!({
        "ok": true,
        "id": "placeholder"
    })))
}

/// List welcome messages endpoint
async fn list_welcomes() -> ActixResult<HttpResponse> {
    Ok(HttpResponse::Ok().json(json!({
        "ok": true,
        "items": []
    })))
}

/// Acknowledge welcome message endpoint
async fn ack_welcome(path: web::Path<String>) -> ActixResult<HttpResponse> {
    let _id = path.into_inner();
    Ok(HttpResponse::Ok().json(json!({
        "ok": true
    })))
}

/// Get missed messages for a user since a timestamp
async fn get_missed_messages(req: web::Json<MissedMessagesRequest>) -> ActixResult<HttpResponse> {
    let archive = match MessageArchive::new().await {
        Ok(archive) => archive,
        Err(e) => {
            return Ok(HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to initialize message archive: {}", e)
            })));
        }
    };

    let limit = req.limit.unwrap_or(100).min(500); // Max 500 messages per request

    match archive.get_missed_messages(&req.pubkey, req.since, limit).await {
        Ok(events) => {
            let messages: Vec<ArchivedMessage> = events.into_iter().map(|event| {
                ArchivedMessage {
                    id: hex::encode(event.id()),
                    kind: event.kind() as u32,
                    content: event.content().to_string(),
                    tags: event.tags().iter().map(|tag| {
                        tag.iter().map(|s| s.to_string()).collect()
                    }).collect(),
                    created_at: event.created_at() as i64,
                    pubkey: hex::encode(event.pubkey()),
                    sig: hex::encode(event.sig()),
                }
            }).collect();

            let count = messages.len() as u32;
            let has_more = count >= limit;

            Ok(HttpResponse::Ok().json(MissedMessagesResponse {
                messages,
                count,
                has_more,
            }))
        }
        Err(e) => {
            Ok(HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to retrieve missed messages: {}", e)
            })))
        }
    }
}

async fn get_group_messages(req: web::Json<GroupMessagesRequest>) -> ActixResult<HttpResponse> {
    let archive = match MessageArchive::new().await {
        Ok(archive) => archive,
        Err(e) => {
            return Ok(HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to initialize message archive: {}", e)
            })));
        }
    };

    let limit = req.limit.unwrap_or(100).min(500); // Max 500 messages per request

    match archive.get_group_messages(&req.group_id, req.since, limit).await {
        Ok(events) => {
            let messages: Vec<ArchivedMessage> = events.into_iter().map(|event| {
                ArchivedMessage {
                    id: hex::encode(event.id()),
                    kind: event.kind() as u32,
                    content: event.content().to_string(),
                    tags: event.tags().iter().map(|tag| {
                        tag.iter().map(|s| s.to_string()).collect()
                    }).collect(),
                    created_at: event.created_at() as i64,
                    pubkey: hex::encode(event.pubkey()),
                    sig: hex::encode(event.sig()),
                }
            }).collect();

            let count = messages.len() as u32;
            let has_more = count >= limit;

            Ok(HttpResponse::Ok().json(MissedMessagesResponse {
                messages,
                count,
                has_more,
            }))
        }
        Err(e) => {
            Ok(HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to retrieve group messages: {}", e)
            })))
        }
    }
}
