//! # oryn-server — HTTP API over the Oryn engine
//!
//! A small, UI-agnostic JSON API so any frontend (web, desktop, GPUI via an
//! HTTP client, CI scripts) can drive the reproducibility & evaluation-integrity
//! engine. Every endpoint is a thin, deterministic wrapper around `oryn-core`.
//!
//! ## Endpoints
//! | Method | Path                  | Body → Response |
//! |--------|-----------------------|-----------------|
//! | GET    | `/api/health`         | → `{status}` |
//! | GET    | `/api/info`           | → versions + compute backend |
//! | POST   | `/api/scan`           | `ScanRequest` → `ContaminationReport` |
//! | POST   | `/api/duplicates`     | `DuplicatesRequest` → `[DuplicatePair]` |
//! | POST   | `/api/eval`           | `EvalRequest` → `EvalReport` |
//! | POST   | `/api/gate`           | `GateRequest` → `RegressionGate` |
//! | POST   | `/api/determinism`    | `{outputs:[String]}` → `DeterminismReport` |
//! | POST   | `/api/integrity`      | `IntegrityReport` → `{verdict, report}` |
//! | POST   | `/api/keygen`         | → `{secret_hex, public_hex}` |
//! | POST   | `/api/attest/seal`    | `SealRequest` → `AttestationChain` |
//! | POST   | `/api/attest/verify`  | `AttestationChain` → `{ok, entries, error?}` |

use axum::{
    extract::Json,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use oryn_core::attest::{AttestationChain, Signer};
use oryn_core::contam::{
    self_duplicates, ContaminationReport, CorpusIndex, Document, DuplicatePair, ScanConfig,
};
use oryn_core::determinism::{analyze_outputs, DeterminismReport};
use oryn_core::eval::{analyze, regression_gate, EvalConfig, EvalReport, EvalRun, RegressionGate};
use oryn_core::report::{IntegrityReport, IntegrityVerdict};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

/// Build the API router (with permissive CORS so browser UIs can call it).
pub fn app() -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/info", get(info))
        .route("/api/scan", post(scan))
        .route("/api/duplicates", post(duplicates))
        .route("/api/eval", post(eval))
        .route("/api/gate", post(gate))
        .route("/api/determinism", post(determinism))
        .route("/api/integrity", post(integrity))
        .route("/api/keygen", post(keygen))
        .route("/api/attest/seal", post(attest_seal))
        .route("/api/attest/verify", post(attest_verify))
        .layer(CorsLayer::permissive())
}

/// Serve the API on an already-bound listener until the process is stopped.
///
/// # Errors
/// Propagates the underlying server I/O error.
pub async fn serve(listener: tokio::net::TcpListener) -> std::io::Result<()> {
    axum::serve(listener, app()).await
}

/// Uniform error envelope.
#[derive(Serialize)]
struct ApiError {
    error: String,
}

/// Concrete fallible-handler result: success JSON or a `400` error envelope.
type ApiResult<T> = std::result::Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn bad_request(msg: impl ToString) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            error: msg.to_string(),
        }),
    )
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

async fn health() -> impl IntoResponse {
    Json(Health { status: "ok" })
}

#[derive(Serialize)]
struct Info {
    name: &'static str,
    engine_version: &'static str,
    compute_backend: &'static str,
    cuda_linked: bool,
}

async fn info() -> impl IntoResponse {
    Json(Info {
        name: "oryn",
        engine_version: oryn_core::VERSION,
        compute_backend: oryn_cuda::backend(),
        cuda_linked: oryn_cuda::cuda_available(),
    })
}

/// Request to contamination-scan an eval set against a corpus.
#[derive(Deserialize)]
pub struct ScanRequest {
    /// Reference corpus.
    pub corpus: Vec<Document>,
    /// Eval set to check.
    pub eval: Vec<Document>,
    /// Optional scan config (defaults applied when absent).
    #[serde(default)]
    pub config: Option<ScanConfig>,
}

async fn scan(Json(req): Json<ScanRequest>) -> ApiResult<ContaminationReport> {
    if req.corpus.is_empty() {
        return Err(bad_request("corpus is empty"));
    }
    let cfg = req.config.unwrap_or_default();
    let index = CorpusIndex::build(&req.corpus, cfg);
    Ok(Json(index.scan(&req.eval)))
}

/// Request to find intra-set duplicates.
#[derive(Deserialize)]
pub struct DuplicatesRequest {
    /// Documents to check for internal near-duplicates.
    pub docs: Vec<Document>,
    /// Optional scan config.
    #[serde(default)]
    pub config: Option<ScanConfig>,
}

#[derive(Serialize)]
struct DuplicatesResponse {
    pairs: Vec<DuplicatePair>,
    count: usize,
}

async fn duplicates(Json(req): Json<DuplicatesRequest>) -> impl IntoResponse {
    let cfg = req.config.unwrap_or_default();
    let pairs = self_duplicates(&req.docs, &cfg);
    let count = pairs.len();
    Json(DuplicatesResponse { pairs, count })
}

/// Request to analyze an eval run.
#[derive(Deserialize)]
pub struct EvalRequest {
    /// The run to analyze.
    pub run: EvalRun,
    /// Optional eval config.
    #[serde(default)]
    pub config: Option<EvalConfig>,
}

async fn eval(Json(req): Json<EvalRequest>) -> ApiResult<EvalReport> {
    let cfg = req.config.unwrap_or_default();
    analyze(&req.run, &cfg).map(Json).map_err(bad_request)
}

/// Request to run a regression gate.
#[derive(Deserialize)]
pub struct GateRequest {
    /// Baseline run.
    pub baseline: EvalRun,
    /// Candidate run.
    pub candidate: EvalRun,
    /// Confidence level (defaults to 0.95).
    #[serde(default = "default_level")]
    pub level: f64,
}

fn default_level() -> f64 {
    0.95
}

async fn gate(Json(req): Json<GateRequest>) -> ApiResult<RegressionGate> {
    regression_gate(&req.baseline, &req.candidate, req.level)
        .map(Json)
        .map_err(bad_request)
}

/// Request to analyze repeated generations.
#[derive(Deserialize)]
pub struct DeterminismRequest {
    /// Repeated outputs.
    pub outputs: Vec<String>,
}

async fn determinism(Json(req): Json<DeterminismRequest>) -> Json<DeterminismReport> {
    Json(analyze_outputs(&req.outputs))
}

#[derive(Serialize)]
struct IntegrityResponse {
    verdict: IntegrityVerdict,
    report: IntegrityReport,
}

async fn integrity(Json(report): Json<IntegrityReport>) -> Json<IntegrityResponse> {
    let verdict = report.verdict();
    Json(IntegrityResponse { verdict, report })
}

#[derive(Serialize)]
struct KeyPair {
    secret_hex: String,
    public_hex: String,
}

async fn keygen() -> Json<KeyPair> {
    let s = Signer::generate();
    Json(KeyPair {
        secret_hex: s.secret_hex(),
        public_hex: s.public_hex(),
    })
}

/// Request to seal arbitrary labeled payloads into a signed chain.
#[derive(Deserialize)]
pub struct SealRequest {
    /// 32-byte secret seed (hex). If absent, a fresh identity is generated.
    #[serde(default)]
    pub seed_hex: Option<String>,
    /// Ordered payloads to seal; each becomes one chain entry.
    pub entries: Vec<SealEntry>,
}

/// One payload to seal.
#[derive(Deserialize)]
pub struct SealEntry {
    /// Entry label.
    pub label: String,
    /// Arbitrary JSON payload (hashed canonically).
    pub payload: serde_json::Value,
}

async fn attest_seal(Json(req): Json<SealRequest>) -> ApiResult<AttestationChain> {
    let signer = match req.seed_hex {
        Some(h) => Signer::from_seed_hex(&h).map_err(bad_request)?,
        None => Signer::generate(),
    };
    let mut chain = AttestationChain::new();
    for e in &req.entries {
        let bytes = serde_json::to_vec(&e.payload).map_err(bad_request)?;
        chain
            .append(&signer, &e.label, &bytes)
            .map_err(bad_request)?;
    }
    Ok(Json(chain))
}

#[derive(Serialize)]
struct VerifyResponse {
    ok: bool,
    entries: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn attest_verify(Json(chain): Json<AttestationChain>) -> Json<VerifyResponse> {
    let entries = chain.entries.len();
    match chain.verify() {
        Ok(()) => Json(VerifyResponse {
            ok: true,
            entries,
            error: None,
        }),
        Err(e) => Json(VerifyResponse {
            ok: false,
            entries,
            error: Some(e.to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn post_json(path: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn health_ok() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn scan_flags_leak() {
        let body = serde_json::json!({
            "corpus": [{"id":"c1","text":"the capital of france is paris eiffel tower champ de mars"}],
            "eval": [
                {"id":"leak","text":"the capital of france is paris eiffel tower champ de mars"},
                {"id":"clean","text":"photosynthesis converts light into chemical energy in plants"}
            ],
            "config": {"ngram_n":3,"normalization":"standard","ngram_threshold":0.5,"jaccard_threshold":0.8,"minhash_perms":64,"lsh_bands":16}
        });
        let (status, json) = post_json("/api/scan", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["contaminated_items"], 1);
        assert_eq!(json["clean_holdout"][0], "clean");
    }

    #[tokio::test]
    async fn eval_reports_ci() {
        let items: Vec<_> = (0..20)
            .map(|i| serde_json::json!({"id": format!("q{i}"), "score": (i % 2) as f64}))
            .collect();
        let body = serde_json::json!({"run": {"name":"t","items": items}});
        let (status, json) = post_json("/api/eval", body).await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["ci"]["high"].as_f64().unwrap() > json["ci"]["low"].as_f64().unwrap());
    }

    #[tokio::test]
    async fn determinism_endpoint() {
        let body = serde_json::json!({"outputs":["a b c","a b d","a b c"]});
        let (status, json) = post_json("/api/determinism", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["deterministic"], false);
        assert_eq!(json["divergence_token"], 2);
    }

    #[tokio::test]
    async fn seal_then_verify() {
        let seal = serde_json::json!({
            "seed_hex": "0707070707070707070707070707070707070707070707070707070707070707",
            "entries": [{"label":"r","payload":{"x":1}}]
        });
        let (status, chain) = post_json("/api/attest/seal", seal).await;
        assert_eq!(status, StatusCode::OK);
        let (status2, verify) = post_json("/api/attest/verify", chain).await;
        assert_eq!(status2, StatusCode::OK);
        assert_eq!(verify["ok"], true);
        assert_eq!(verify["entries"], 1);
    }
}
