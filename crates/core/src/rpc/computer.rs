//! Computer assistant (`computer.*`, ADR-0035). Thin adapters over
//! [`orkestr_computer::ComputerService`]; the policy keeps the whole area
//! local-only. [`install`] wires the AI planner to the provider stack (the
//! AI Manager's model, then routing — the offline Mock answers planner
//! prompts with a deterministic plan).

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use futures::StreamExt;
use orkestr_computer::{ComputerError, ComputerResult, PlanModel};
use orkestr_domain::AiPolicy;
use orkestr_protocol::methods::computer::{
    self as m, HistoryResult, MatchResult, PlanResult, ProcessesResult, SetResult, SetsResult, WindowsResult,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::RpcError;
use orkestr_providers::{ChatEvent, ChatMessage, ChatRequest, RequiredCapabilities, RouteRequest};
use tokio_util::sync::CancellationToken;

use super::{RpcContext, RpcRouter};
use crate::Services;

fn lang(l: &str) -> &str {
    if l.is_empty() {
        "en"
    } else {
        l
    }
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Status, _, _>(|ctx: RpcContext, _: Empty| async move {
            ctx.services.computer.refresh_apps();
            Ok::<_, RpcError>(ctx.services.computer.status())
        })
        .method::<m::Handle, _, _>(|ctx: RpcContext, p: m::HandleParams| async move {
            let source = if p.source.is_empty() { "text" } else { p.source.as_str() };
            Ok(ctx.services.computer.handle(&p.text, lang(&p.lang), p.plan_unknown, source).await?)
        })
        .method::<m::Match, _, _>(|ctx: RpcContext, p: m::MatchParams| async move {
            let limit = p.limit.unwrap_or(5) as usize;
            let (matches, elapsed_us) = ctx.services.computer.match_text(&p.text, lang(&p.lang), limit).await;
            Ok::<_, RpcError>(MatchResult { matches, elapsed_us })
        })
        .method::<m::Run, _, _>(|ctx: RpcContext, p: m::RunComputerCommandParams| async move {
            Ok(ctx.services.computer.run_command(p.set_id.as_deref(), &p.command_id, p.slots, lang(&p.lang)).await?)
        })
        .method::<m::Execute, _, _>(|ctx: RpcContext, p: m::ExecuteParams| async move {
            Ok(ctx.services.computer.execute_steps(&p.title, p.steps, p.slots).await?)
        })
        .method::<m::Confirm, _, _>(|ctx: RpcContext, p: m::ConfirmParams| async move {
            Ok(ctx.services.computer.confirm(&p.pending_id, p.approve).await?)
        })
        .method::<m::Plan, _, _>(|ctx: RpcContext, p: m::PlanParams| async move {
            let plan = ctx
                .services
                .computer
                .plan(&p.text, p.plan_id.as_deref(), p.correction.as_deref(), lang(&p.lang))
                .await?;
            Ok::<_, RpcError>(PlanResult { plan })
        })
        .method::<m::ApprovePlan, _, _>(|ctx: RpcContext, p: m::PlanIdParams| async move {
            Ok(ctx.services.computer.approve_plan(&p.plan_id).await?)
        })
        .method::<m::RememberPlan, _, _>(|ctx: RpcContext, p: m::RememberPlanParams| async move {
            Ok(ctx.services.computer.remember_plan(&p.plan_id, p.phrases, p.title, p.set_id)?)
        })
        .method::<m::Catalogue, _, _>(|ctx: RpcContext, p: m::CatalogueParams| async move {
            Ok::<_, RpcError>(ctx.services.computer.catalogue(&p))
        })
        .method::<m::ListSets, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok::<_, RpcError>(SetsResult { sets: ctx.services.computer.list_sets() })
        })
        .method::<m::GetSet, _, _>(|ctx: RpcContext, p: m::SetIdParams| async move {
            Ok(SetResult { set: ctx.services.computer.get_set(&p.set_id)? })
        })
        .method::<m::SaveSet, _, _>(|ctx: RpcContext, p: m::SaveSetParams| async move {
            Ok(SetResult { set: ctx.services.computer.save_set(p.set)? })
        })
        .method::<m::DeleteSet, _, _>(|ctx: RpcContext, p: m::SetIdParams| async move {
            ctx.services.computer.delete_set(&p.set_id)?;
            Ok(Empty {})
        })
        .method::<m::SetEnabled, _, _>(|ctx: RpcContext, p: m::SetEnabledParams| async move {
            Ok(SetResult { set: ctx.services.computer.set_enabled(&p.set_id, p.enabled)? })
        })
        .method::<m::ExportSet, _, _>(|ctx: RpcContext, p: m::SetIdParams| async move {
            Ok(ctx.services.computer.export_set(&p.set_id)?)
        })
        .method::<m::ImportSet, _, _>(|ctx: RpcContext, p: m::ImportSetParams| async move {
            Ok(SetResult { set: ctx.services.computer.import_set(&p.json, p.replace)? })
        })
        .method::<m::Windows, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok(WindowsResult { windows: ctx.services.computer.windows().await? })
        })
        .method::<m::Processes, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok(ProcessesResult { processes: ctx.services.computer.processes().await? })
        })
        .method::<m::Stop, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok::<_, RpcError>(ctx.services.computer.stop().await)
        })
        .method::<m::History, _, _>(|ctx: RpcContext, p: m::HistoryParams| async move {
            Ok::<_, RpcError>(HistoryResult { entries: ctx.services.computer.history(p.limit.unwrap_or(20) as usize) })
        });
}

/// Planner model over the provider stack.
struct CoreModel(Weak<Services>);

#[async_trait]
impl PlanModel for CoreModel {
    async fn complete(
        &self,
        system: String,
        user: String,
        cancel: CancellationToken,
    ) -> ComputerResult<(String, String)> {
        let s = self.0.upgrade().ok_or_else(|| ComputerError::Planner("core is shutting down".into()))?;
        match manager_complete(&s, system, user, 1_500, false, cancel.clone()).await {
            Ok(r) => Ok(r),
            Err(_) if cancel.is_cancelled() => Err(ComputerError::Stopped),
            Err(e) => Err(ComputerError::Planner(e)),
        }
    }
}

/// One-shot completion with the AI Manager's model (then routing). With
/// `real_only`, the offline Mock provider is skipped (its canned answers are
/// not useful outside the planner).
///
/// # Errors
/// No model available, or the model call failed (message).
pub(crate) async fn manager_complete(
    s: &Services,
    system: String,
    user: String,
    max_output_tokens: u32,
    real_only: bool,
    cancel: CancellationToken,
) -> Result<(String, String), String> {
    let mut chain = Vec::new();
    if let Some(m) = s.providers.manager_config().await.ok().and_then(|c| c.model) {
        chain.push(m);
    }
    let routed = s
        .providers
        .route(&RouteRequest {
            purpose: Some("Manager".into()),
            required: RequiredCapabilities::default(),
            preferred: chain.first().cloned(),
            fallback_chain: Vec::new(),
            policy: AiPolicy::default(),
            max_cost_usd: None,
            est_input_tokens: 3_000,
            max_output_tokens: u64::from(max_output_tokens),
            exclude: Vec::new(),
        })
        .await
        .unwrap_or_default();
    for r in routed {
        if !chain.contains(&r) {
            chain.push(r);
        }
    }
    if real_only {
        let mock: Vec<_> = s
            .providers
            .list()
            .into_iter()
            .filter(|v| v.provider.kind == orkestr_domain::ProviderKind::Mock)
            .map(|v| v.provider.id)
            .collect();
        chain.retain(|m| !mock.contains(&m.provider_id));
    }
    let head = chain.first().cloned().ok_or_else(|| "no chat model is available (connect a provider)".to_owned())?;
    let mut chat = ChatRequest::new(head.model.clone(), vec![ChatMessage::user(user)]);
    chat.system = Some(system);
    chat.max_output_tokens = Some(max_output_tokens);
    let mut stream = s.providers.chat(chain, chat, None, cancel.clone());
    let (mut text, mut model) = (String::new(), head);
    while let Some(ev) = stream.next().await {
        match ev {
            ChatEvent::TextDelta(t) => text.push_str(&t),
            ChatEvent::Switched { to, .. } => {
                text.clear();
                model = to;
            }
            ChatEvent::Done { .. } => return Ok((text, model.model)),
            ChatEvent::Error(e) => return Err(format!("model call failed: {e}")),
            ChatEvent::Usage(_) | ChatEvent::ToolCall { .. } => {}
        }
    }
    Err("the model stream ended unexpectedly".into())
}

/// Wires the planner model and starts indexing Start-menu apps.
pub fn install(services: &Arc<Services>) {
    services.computer.set_model(Arc::new(CoreModel(Arc::downgrade(services))));
    services.computer.refresh_apps();
}
