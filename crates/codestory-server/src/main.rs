use anyhow::{Context, Result};
use async_stream::stream;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post},
};
use clap::Parser;
use codestory_api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentResponseSectionDto, ApiError,
    AppEventPayload, EdgeId, EdgeKind, EdgeOccurrencesRequest, GraphArtifactDto, GraphEdgeDto,
    GraphNodeDto, GraphRequest, GraphResponse, IndexMode, ListChildrenSymbolsRequest,
    ListRootSymbolsRequest, NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind,
    NodeOccurrencesRequest, OpenProjectRequest, ProjectSummary, ReadFileTextRequest,
    ReadFileTextResponse, SearchHit, SearchRequest, SetUiLayoutRequest, SourceOccurrenceDto,
    StartIndexingRequest, StorageStatsDto, SymbolSummaryDto, TrailCallerScope, TrailConfigDto,
    TrailDirection, TrailMode, WriteFileResponse, WriteFileTextRequest,
};
use codestory_app::AppController;
use serde::Serialize;
use specta::TypeCollection;
use specta_typescript::Typescript;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value_t = 7878)]
    port: u16,

    #[arg(long)]
    project: Option<PathBuf>,

    #[arg(long, default_value = "codestory-ui/src/generated/api.ts")]
    types_out: PathBuf,

    #[arg(long)]
    types_only: bool,

    #[arg(long, default_value = "codestory-ui/dist")]
    frontend_dist: PathBuf,

    #[arg(long)]
    skip_types_gen: bool,
}

#[derive(Clone)]
struct ServerState {
    controller: AppController,
    events_tx: broadcast::Sender<AppEventPayload>,
}

#[derive(Debug)]
struct HttpError(ApiError);

type ApiResult<T> = Result<Json<T>, HttpError>;
type ApiEmptyResult = Result<StatusCode, HttpError>;

impl From<ApiError> for HttpError {
    fn from(value: ApiError) -> Self {
        Self(value)
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self.0.code.as_str() {
            "invalid_argument" => StatusCode::BAD_REQUEST,
            "not_found" => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    if args.types_only {
        write_typescript_bindings(&args.types_out)?;
        info!(path = %args.types_out.display(), "Generated frontend API types");
        return Ok(());
    }

    if !args.skip_types_gen {
        write_typescript_bindings(&args.types_out)?;
        info!(path = %args.types_out.display(), "Generated frontend API types");
    }

    let controller = AppController::new();
    if let Some(project) = args.project {
        controller
            .open_project(OpenProjectRequest {
                path: project.to_string_lossy().to_string(),
            })
            .map_err(|e| anyhow::anyhow!("{:?}", e))?;
    }

    let (events_tx, _) = broadcast::channel::<AppEventPayload>(512);
    {
        let events_rx = controller.events();
        let events_tx = events_tx.clone();
        std::thread::spawn(move || {
            while let Ok(event) = events_rx.recv() {
                let _ = events_tx.send(event);
            }
        });
    }

    let state = Arc::new(ServerState {
        controller,
        events_tx,
    });

    let api_routes = Router::new()
        .route("/health", get(health))
        .route("/events", get(events_stream))
        .route("/open-project", post(open_project))
        .route("/index/start", post(start_indexing))
        .route("/search", post(search))
        .route("/agent/ask", post(agent_ask))
        .route("/graph/neighborhood", post(graph_neighborhood))
        .route("/graph/trail", post(graph_trail))
        .route("/node/details", post(node_details))
        .route("/node/occurrences", post(node_occurrences))
        .route("/edge/occurrences", post(edge_occurrences))
        .route("/file/read", post(read_file_text))
        .route("/file/write-text", post(write_file_text))
        .route("/ui-layout", get(get_ui_layout).post(set_ui_layout))
        .route("/explorer/root", get(list_root_symbols))
        .route("/explorer/children/{node_id}", get(list_children_symbols));

    let mut app = Router::new()
        .nest("/api", api_routes)
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

    if args.frontend_dist.exists() {
        let spa = ServeDir::new(&args.frontend_dist)
            .not_found_service(ServeFile::new(args.frontend_dist.join("index.html")));
        app = app.fallback_service(spa);
    }

    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .context("Failed to parse server address")?;
    info!(%addr, "Starting CodeStory server");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn collect_types() -> TypeCollection {
    let mut types = TypeCollection::default();

    types
        .register::<ApiError>()
        .register::<AppEventPayload>()
        .register::<NodeId>()
        .register::<EdgeId>()
        .register::<IndexMode>()
        .register::<NodeKind>()
        .register::<EdgeKind>()
        .register::<TrailMode>()
        .register::<TrailDirection>()
        .register::<TrailCallerScope>()
        .register::<OpenProjectRequest>()
        .register::<StorageStatsDto>()
        .register::<ProjectSummary>()
        .register::<StartIndexingRequest>()
        .register::<SearchRequest>()
        .register::<SearchHit>()
        .register::<ListRootSymbolsRequest>()
        .register::<ListChildrenSymbolsRequest>()
        .register::<SymbolSummaryDto>()
        .register::<GraphRequest>()
        .register::<GraphNodeDto>()
        .register::<GraphEdgeDto>()
        .register::<GraphResponse>()
        .register::<TrailConfigDto>()
        .register::<NodeDetailsRequest>()
        .register::<NodeDetailsDto>()
        .register::<NodeOccurrencesRequest>()
        .register::<EdgeOccurrencesRequest>()
        .register::<SourceOccurrenceDto>()
        .register::<ReadFileTextRequest>()
        .register::<ReadFileTextResponse>()
        .register::<WriteFileTextRequest>()
        .register::<WriteFileResponse>()
        .register::<SetUiLayoutRequest>()
        .register::<AgentAskRequest>()
        .register::<AgentCitationDto>()
        .register::<AgentResponseSectionDto>()
        .register::<GraphArtifactDto>()
        .register::<AgentAnswerDto>();

    types
}

fn write_typescript_bindings(path: &StdPath) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create type output directory: {}",
                parent.display()
            )
        })?;
    }

    let types = collect_types();
    Typescript::default()
        .export_to(path, &types)
        .with_context(|| format!("Failed to write TypeScript bindings to {}", path.display()))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn events_stream(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let mut rx = state.events_tx.subscribe();
    let stream = stream! {
        loop {
            match rx.recv().await {
                Ok(event_payload) => {
                    let data = serde_json::to_string(&event_payload).unwrap_or_else(|_| {
                        "{\"type\":\"StatusUpdate\",\"data\":{\"message\":\"serialization_error\"}}".to_string()
                    });
                    yield Ok::<Event, Infallible>(Event::default().event("app_event").data(data));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

async fn open_project(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<OpenProjectRequest>,
) -> ApiResult<ProjectSummary> {
    state
        .controller
        .open_project(req)
        .map(Json)
        .map_err(Into::into)
}

async fn start_indexing(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<StartIndexingRequest>,
) -> ApiEmptyResult {
    state.controller.start_indexing(req)?;
    Ok(StatusCode::ACCEPTED)
}

async fn search(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<SearchRequest>,
) -> ApiResult<Vec<SearchHit>> {
    state.controller.search(req).map(Json).map_err(Into::into)
}

async fn agent_ask(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<AgentAskRequest>,
) -> ApiResult<AgentAnswerDto> {
    state
        .controller
        .agent_ask(req)
        .map(Json)
        .map_err(Into::into)
}

async fn graph_neighborhood(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<GraphRequest>,
) -> ApiResult<GraphResponse> {
    state
        .controller
        .graph_neighborhood(req)
        .map(Json)
        .map_err(Into::into)
}

async fn graph_trail(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<TrailConfigDto>,
) -> ApiResult<GraphResponse> {
    state
        .controller
        .graph_trail(req)
        .map(Json)
        .map_err(Into::into)
}

async fn node_details(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<NodeDetailsRequest>,
) -> ApiResult<NodeDetailsDto> {
    state
        .controller
        .node_details(req)
        .map(Json)
        .map_err(Into::into)
}

async fn node_occurrences(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<NodeOccurrencesRequest>,
) -> ApiResult<Vec<SourceOccurrenceDto>> {
    state
        .controller
        .node_occurrences(req)
        .map(Json)
        .map_err(Into::into)
}

async fn edge_occurrences(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<EdgeOccurrencesRequest>,
) -> ApiResult<Vec<SourceOccurrenceDto>> {
    state
        .controller
        .edge_occurrences(req)
        .map(Json)
        .map_err(Into::into)
}

async fn read_file_text(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<ReadFileTextRequest>,
) -> ApiResult<ReadFileTextResponse> {
    state
        .controller
        .read_file_text(req)
        .map(Json)
        .map_err(Into::into)
}

async fn write_file_text(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<WriteFileTextRequest>,
) -> ApiResult<WriteFileResponse> {
    state
        .controller
        .write_file_text(req)
        .map(Json)
        .map_err(Into::into)
}

async fn get_ui_layout(State(state): State<Arc<ServerState>>) -> ApiResult<Option<String>> {
    state
        .controller
        .get_ui_layout()
        .map(Json)
        .map_err(Into::into)
}

async fn set_ui_layout(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<SetUiLayoutRequest>,
) -> ApiEmptyResult {
    state.controller.set_ui_layout(req)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_root_symbols(
    State(state): State<Arc<ServerState>>,
    Query(req): Query<ListRootSymbolsRequest>,
) -> ApiResult<Vec<SymbolSummaryDto>> {
    state
        .controller
        .list_root_symbols(req)
        .map(Json)
        .map_err(Into::into)
}

async fn list_children_symbols(
    State(state): State<Arc<ServerState>>,
    Path(node_id): Path<String>,
) -> ApiResult<Vec<SymbolSummaryDto>> {
    state
        .controller
        .list_children_symbols(ListChildrenSymbolsRequest {
            parent_id: NodeId(node_id),
        })
        .map(Json)
        .map_err(Into::into)
}
