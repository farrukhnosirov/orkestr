//! Service-level tests with the recording backend and a scripted planner
//! model: catalogue matching → run, confirmation flow through the approvals
//! service, AI plan → correction → approve → remember → run by phrase,
//! user sets (import / export / enable / context), global stop, audit.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use orkestr_computer::backend::{MockBackend, MockCall};
use orkestr_computer::{ComputerDeps, ComputerError, ComputerResult, ComputerService, PlanModel, PLANNER_MARKER};
use orkestr_events::EventBus;
use orkestr_protocol::methods::computer::{
    CatalogueParams, ClientEffect, ComputerAction, HandleResult, RunOutcome, SafetyLevel, WindowOp,
};
use orkestr_security::{AuditQuery, Security, SecurityConfig};
use orkestr_storage::Db;

/// Scripted planner: notepad + typed text; corrections append to the text.
struct FakeModel;

#[async_trait]
impl PlanModel for FakeModel {
    async fn complete(&self, system: String, user: String, _c: CancellationToken) -> ComputerResult<(String, String)> {
        assert!(system.contains(PLANNER_MARKER));
        let task = user.lines().find_map(|l| l.strip_prefix("Task: ")).unwrap_or("").to_owned();
        if task.contains("impossible") {
            return Ok(("{\"explanation\":\"cannot do that\",\"steps\":[]}".into(), "fake/model".into()));
        }
        let mut text = "Shopping: milk, bread".to_owned();
        for c in user.lines().filter_map(|l| l.strip_prefix("Correction: ")) {
            text.push_str(&format!(", {c}"));
        }
        let plan = serde_json::json!({
            "explanation": format!("Opens Notepad and writes the list for: {task}"),
            "steps": [
                {"type": "Launch", "target": "notepad"},
                {"type": "Wait", "ms": 10},
                {"type": "TypeText", "text": text}
            ]
        });
        Ok((format!("```json\n{plan}\n```"), "fake/model".into()))
    }
}

struct Fx {
    svc: Arc<ComputerService>,
    backend: Arc<MockBackend>,
    security: Arc<Security>,
    _dir: tempfile::TempDir,
}

async fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("t.db")).unwrap();
    let mut migs = orkestr_storage::storage_migrations();
    migs.extend(orkestr_events::events_migrations());
    migs.extend(orkestr_security::migrations());
    db.migrate(migs).await.unwrap();
    let events = EventBus::in_memory(1024);
    let security = Arc::new(
        Security::open(SecurityConfig {
            db,
            events: events.clone(),
            data_dir: dir.path().join("data"),
            credentials: None,
            secret_backend: orkestr_security::SecretBackendChoice::Memory,
            guard: orkestr_security::GuardConfig::default(),
        })
        .await
        .unwrap(),
    );
    let backend = Arc::new(MockBackend::default());
    let svc = ComputerService::new(ComputerDeps {
        events,
        security: Arc::clone(&security),
        data_dir: dir.path().join("computer"),
        backend: backend.clone(),
    });
    svc.set_model(Arc::new(FakeModel));
    Fx { svc, backend, security, _dir: dir }
}

fn executed(r: &HandleResult) -> &orkestr_protocol::methods::computer::RunReport {
    match r {
        HandleResult::Executed { run, .. } => run,
        other => panic!("expected Executed, got {other:?}"),
    }
}

#[tokio::test]
async fn catalogue_commands_run_by_phrase_in_both_languages() {
    let f = fx().await;
    assert!(f.svc.status().catalogue_commands >= 1000);

    let r = f.svc.handle("открой блокнот", "ru", false, "text").await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    assert_eq!(f.backend.take_calls(), vec![MockCall::Open("notepad.exe".into(), None)]);

    f.backend.add_window("0x10", "Untitled - Notepad", "Notepad.exe", true);
    let r = f.svc.handle("напечатай Привет, мир", "ru", false, "voice").await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    assert!(f.backend.take_calls().contains(&MockCall::Type("Привет, мир".into())));

    let r = f.svc.handle("scroll down 3", "en", false, "text").await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    let r = f.svc.handle("прокрути вниз на пять", "ru", false, "text").await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    let calls = f.backend.take_calls();
    assert!(calls.iter().any(|c| matches!(c, MockCall::Scroll(_, 3, _))));
    assert!(calls.iter().any(|c| matches!(c, MockCall::Scroll(_, 5, _))));

    let r = f.svc.handle("open bluetooth settings", "en", false, "text").await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    assert_eq!(f.backend.take_calls(), vec![MockCall::Open("ms-settings:bluetooth".into(), None)]);

    let r = f.svc.handle("найди на ютубе котики", "ru", false, "text").await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    assert!(
        matches!(&f.backend.take_calls()[0], MockCall::Open(u, _) if u.starts_with("https://www.youtube.com/results"))
    );

    let r = f.svc.handle("режим ассистента", "ru", false, "voice").await.unwrap();
    assert!(executed(&r).effects.iter().any(|e| matches!(e, ClientEffect::SetMode { .. })));

    // matcher speed on the full catalogue
    let (m, us) = f.svc.match_text("сверни все окна", "ru", 3).await;
    assert_eq!(m[0].command_id, "win.desktop");
    assert!(us < 500_000, "matcher took {us} µs");
}

#[tokio::test]
async fn closing_without_saving_needs_confirmation_and_audits() {
    let f = fx().await;
    f.backend.add_window("0x20", "*notes - Notepad", "Notepad.exe", true);
    let r = f.svc.handle("закрой блокнот без сохранения", "ru", false, "voice").await.unwrap();
    let HandleResult::NeedsConfirmation { pending, .. } = r else { panic!("{r:?}") };
    assert_eq!(pending.reasons[0].code, "closeDiscard");
    let approval = pending.approval_id.expect("approval in the inbox");
    assert!(f.backend.take_calls().is_empty(), "nothing ran before the confirmation");
    let a = f.security.approvals.get(approval).await.unwrap().unwrap();
    assert_eq!(a.status, orkestr_domain::ApprovalStatus::Pending);

    let r = f.svc.confirm(&pending.pending_id, true).await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    assert!(f.backend.take_calls().contains(&MockCall::Window(Some("0x20".into()), WindowOp::Close)));
    assert!(f.backend.windows.lock().is_empty());

    // rejection
    f.backend.add_window("0x21", "*x - Notepad", "Notepad.exe", true);
    let r = f.svc.handle("close notepad without saving", "en", false, "text").await.unwrap();
    let HandleResult::NeedsConfirmation { pending, .. } = r else { panic!() };
    let r = f.svc.confirm(&pending.pending_id, false).await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Rejected);
    assert_eq!(f.backend.windows.lock().len(), 1);

    // kill is dangerous; system processes are forbidden
    let r = f.svc.handle("kill process chrome", "en", false, "text").await.unwrap();
    assert!(
        matches!(r, HandleResult::NeedsConfirmation { ref pending, .. } if pending.reasons[0].level == SafetyLevel::Dangerous)
    );
    assert!(matches!(
        f.svc.handle("убей процесс explorer", "ru", false, "text").await,
        Err(ComputerError::Forbidden(_))
    ));

    let audit = f.security.audit.query(AuditQuery::default()).await.unwrap();
    let actions: Vec<&str> = audit.iter().map(|r| r.event.action.as_str()).collect();
    assert!(actions.iter().filter(|a| **a == "computer.run").count() >= 4, "{actions:?}");
}

#[tokio::test]
async fn approvals_inbox_resolution_also_runs_the_command() {
    let f = fx().await;
    f.backend.add_window("0x30", "Doc - Word", "WINWORD.EXE", true);
    let r = f.svc.handle("close word without saving", "en", false, "text").await.unwrap();
    let HandleResult::NeedsConfirmation { pending, .. } = r else { panic!() };
    f.security
        .approvals
        .resolve(pending.approval_id.unwrap(), orkestr_security::ApprovalDecision::Approve, None, f.security.user())
        .await
        .unwrap();
    for _ in 0..100 {
        if f.backend.windows.lock().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(f.backend.windows.lock().is_empty(), "approved from the inbox → executed");
}

#[tokio::test]
async fn unknown_command_plan_correct_approve_remember_and_rerun() {
    let f = fx().await;
    let r = f.svc.handle("составь список покупок", "ru", true, "voice").await.unwrap();
    let HandleResult::Proposed { plan } = r else { panic!("{r:?}") };
    assert_eq!(plan.steps.len(), 3);
    assert_eq!(plan.model, "fake/model");
    assert!(f.backend.take_calls().is_empty(), "plans never run before approval");

    let plan = f.svc.plan("", Some(&plan.plan_id), Some("eggs"), "ru").await.unwrap();
    assert_eq!(plan.corrections, ["eggs"]);
    assert!(matches!(&plan.steps[2], ComputerAction::TypeText { text } if text.ends_with(", eggs")));

    let r = f.svc.approve_plan(&plan.plan_id).await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    let calls = f.backend.take_calls();
    assert!(calls.contains(&MockCall::Open("notepad.exe".into(), None)));
    assert!(calls.contains(&MockCall::Type("Shopping: milk, bread, eggs".into())));

    let r = f.svc.handle("запомни это как список покупок", "ru", true, "voice").await.unwrap();
    let HandleResult::Remembered { set_id, command } = r else { panic!("{r:?}") };
    assert_eq!(set_id, "my-commands");
    assert_eq!(command.phrases, ["список покупок"]);

    let r = f.svc.handle("список покупок", "ru", true, "voice").await.unwrap();
    let run = executed(&r);
    assert_eq!(run.command_id.as_deref(), Some(command.id.as_str()));
    assert!(f.backend.take_calls().contains(&MockCall::Type("Shopping: milk, bread, eggs".into())));

    // the user set survives a restart of the service (same data dir)
    assert!(f.svc.list_sets().iter().any(|s| s.id == "my-commands" && s.command_count == 1));

    let r = f.svc.handle("do something impossible", "en", true, "text").await.unwrap();
    assert!(matches!(r, HandleResult::NotUnderstood { reason: Some(ref why), .. } if why.contains("cannot do that")));
}

#[tokio::test]
async fn user_sets_import_export_enable_and_context() {
    let f = fx().await;
    let doc = serde_json::json!({
        "format": "orkestr-command-set", "version": 1,
        "set": {"id": "browser-extra", "name": "Browser extras",
            "context": {"processes": ["chrome"], "titleContains": []},
            "commands": [{"id": "pin", "phrases": ["закрепи вкладку", "pin tab"],
                "steps": [{"type": "ClickElement", "name": "Pin"}]}]}
    })
    .to_string();
    let set = f.svc.import_set(&doc, false).unwrap();
    assert_eq!(set.id, "browser-extra");
    // context: not active without a chrome window
    let r = f.svc.handle("pin tab", "en", false, "text").await.unwrap();
    assert!(matches!(r, HandleResult::NotUnderstood { .. }), "{r:?}");
    f.backend.add_window("0x40", "GitHub - Google Chrome", "chrome.exe", true);
    let r = f.svc.handle("pin tab", "en", false, "text").await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    assert!(f.backend.take_calls().contains(&MockCall::ClickElement("Pin".into())));

    f.svc.set_enabled("browser-extra", false).unwrap();
    assert!(matches!(f.svc.handle("pin tab", "en", false, "text").await.unwrap(), HandleResult::NotUnderstood { .. }));
    f.svc.set_enabled("media", false).unwrap();
    assert!(!f.svc.list_sets().iter().find(|s| s.id == "media").unwrap().enabled);

    let exported = f.svc.export_set("browser-extra").unwrap();
    let again = f.svc.import_set(&exported.json, false).unwrap();
    assert_eq!(again.id, "browser-extra-2", "imports never overwrite silently");
    assert!(f.svc.save_set(f.svc.get_set("apps").unwrap()).is_err(), "built-in sets are read-only");
    assert!(f.svc.delete_set("apps").is_err());
    f.svc.delete_set("browser-extra-2").unwrap();

    let page =
        f.svc.catalogue(&CatalogueParams { query: "блокнот".into(), lang: "ru".into(), ..Default::default() });
    assert!(page.total >= 5 && page.commands.iter().any(|c| c.command_id == "app.notepad.open"));
    assert!(page.categories.iter().map(|c| c.count).sum::<u32>() >= 1000);
}

#[tokio::test]
async fn global_stop_cancels_runs_pending_and_timers() {
    let f = fx().await;
    let r = f.svc.handle("таймер на 5 минут", "ru", false, "voice").await.unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Succeeded);
    let r = f.svc.handle("выключи компьютер", "ru", false, "voice").await.unwrap();
    assert!(matches!(r, HandleResult::NeedsConfirmation { .. }));
    let svc = Arc::clone(&f.svc);
    let long = tokio::spawn(async move {
        svc.execute_steps("wait", vec![ComputerAction::Wait { ms: 60_000.into() }], BTreeMap::new()).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let s = f.svc.stop().await;
    assert_eq!((s.pending_cancelled, s.timers_cancelled), (1, 1));
    let r = long.await.unwrap().unwrap();
    assert_eq!(executed(&r).outcome, RunOutcome::Stopped);
    assert!(!f.backend.take_calls().iter().any(|c| matches!(c, MockCall::System(_))), "shutdown never ran");
    assert!(f.svc.history(50).len() >= 3);
}
