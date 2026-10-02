import type { Approval, PermissionRuleRecord } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import { ApprovalsBadge, ApprovalsDrawer } from "./ApprovalsInbox";
import { EmergencyConfirmDialog, EmergencyStopButton } from "./EmergencyStop";
import {
  activeFlags,
  dangerLabel,
  EMPTY_FLAGS,
  relativeTime,
  removeApproval,
  riskTone,
  scopeLabel,
  shortId,
  sortRules,
  upsertApproval,
  validateSecretName,
} from "./logic";
import { registerSecurityFeature } from "./register";
import { useSecurityStore } from "./store";

function approval(id: string, requestedAt: string, status: Approval["status"] = "Pending"): Approval {
  return {
    id,
    projectId: null,
    taskId: null,
    agentId: "0190f0b0-0000-7000-8000-00000000abcd",
    toolCallId: null,
    capability: "Terminal",
    actionSummary: `Run git push --force (${id})`,
    risk: "High",
    reason: "destructive git operation",
    status,
    requestedAt,
    expiresAt: null,
    resolvedAt: null,
    resolvedBy: null,
    resolutionNote: null,
    commandPreview: "git push --force origin main",
    danger: "GitDestructive",
  };
}

function rule(scope: PermissionRuleRecord["rule"]["scope"], capability: PermissionRuleRecord["rule"]["capability"], createdAt: string): PermissionRuleRecord {
  return { id: `${scope.scope}-${capability}-${createdAt}`, rule: { capability, effect: "Allow", resource: null, scope }, note: null, createdAt, expiresAt: null };
}

afterEach(() => {
  cleanup();
  useSecurityStore.setState({ pending: [], pendingLoaded: false, inboxOpen: false, confirmStopOpen: false, flags: EMPTY_FLAGS, toasts: [] });
});

describe("security logic", () => {
  it("keeps only pending approvals, newest first", () => {
    let list = upsertApproval([], approval("a", "2026-10-02T10:00:00Z"));
    list = upsertApproval(list, approval("b", "2026-10-02T11:00:00Z"));
    expect(list.map((a) => a.id)).toEqual(["b", "a"]);
    list = upsertApproval(list, approval("a", "2026-10-02T10:00:00Z", "Approved"));
    expect(list.map((a) => a.id)).toEqual(["b"]);
    expect(removeApproval(list, "b")).toEqual([]);
  });

  it("sorts rules narrowest scope first", () => {
    const sorted = sortRules([
      rule({ scope: "Global" }, "Read", "1"),
      rule({ scope: "Task", taskId: "t" }, "Write", "2"),
      rule({ scope: "Project", projectId: "p" }, "Network", "3"),
      rule({ scope: "Agent", agentId: "a" }, "Git", "4"),
    ]);
    expect(sorted.map((r) => r.rule.scope.scope)).toEqual(["Task", "Agent", "Project", "Global"]);
  });

  it("labels and tones", () => {
    expect(riskTone("Critical")).toBe("danger");
    expect(riskTone("Low")).toBe("success");
    expect(dangerLabel("GitDestructive")).toBe("Destructive git");
    expect(scopeLabel({ scope: "Project", projectId: "p1" }, () => "Payroll")).toBe("Project · Payroll");
    expect(shortId("0190f0b0-0000-7000-8000-00000000abcd")).toBe("…00abcd");
    expect(activeFlags({ ...EMPTY_FLAGS, allStopped: true, vaultLocked: true })).toEqual(["All AI stopped", "Vault locked"]);
    expect(activeFlags(EMPTY_FLAGS)).toEqual([]);
  });

  it("validates secret names like the core", () => {
    expect(validateSecretName("OPENAI_API_KEY")).toBeNull();
    expect(validateSecretName("db.url-1")).toBeNull();
    expect(validateSecretName("")).not.toBeNull();
    expect(validateSecretName("1abc")).not.toBeNull();
    expect(validateSecretName("has space")).not.toBeNull();
  });

  it("formats relative time", () => {
    const now = Date.parse("2026-10-02T12:00:00Z");
    expect(relativeTime("2026-10-02T11:59:50Z", now)).toBe("just now");
    expect(relativeTime("2026-10-02T11:55:00Z", now)).toBe("5 min ago");
    expect(relativeTime("2026-10-02T09:00:00Z", now)).toBe("3 h ago");
    expect(relativeTime("nope", now)).toBe("");
  });
});

describe("security UI", () => {
  it("badge shows the live pending count and opens the drawer", () => {
    render(
      <>
        <ApprovalsBadge />
        <ApprovalsDrawer />
      </>,
    );
    expect(screen.getByRole("button", { name: /Approvals inbox \(empty\)/ })).toBeTruthy();
    act(() => {
      useSecurityStore.getState().setPending([approval("a", "2026-10-02T10:00:00Z"), approval("b", "2026-10-02T11:00:00Z")]);
    });
    const badge = screen.getByRole("button", { name: /2 pending/ });
    fireEvent.click(badge);
    expect(screen.getByRole("dialog", { name: "Approvals" })).toBeTruthy();
    expect(screen.getAllByRole("article")).toHaveLength(2);
    expect(screen.getAllByText("Destructive git")).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: /Approve/ })).toHaveLength(2);
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Approvals" }), { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Approvals" })).toBeNull();
  });

  it("drawer shows an empty state", () => {
    act(() => {
      useSecurityStore.getState().setPending([]);
      useSecurityStore.getState().setInboxOpen(true);
    });
    render(<ApprovalsDrawer />);
    expect(screen.getByText("All clear")).toBeTruthy();
  });

  it("emergency stop asks for confirmation and reflects the stopped state", () => {
    render(
      <>
        <EmergencyStopButton />
        <EmergencyConfirmDialog />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: /Emergency stop: stop all AI/ }));
    expect(screen.getByRole("alertdialog", { name: "Stop all AI?" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    act(() => useSecurityStore.getState().setFlags({ ...EMPTY_FLAGS, allStopped: true }));
    fireEvent.click(screen.getByRole("button", { name: /Emergency stop engaged/ }));
    expect(screen.getByRole("alertdialog", { name: "All AI is stopped" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Re-arm/ })).toBeTruthy();
  });

  it("registers commands and settings sections once", () => {
    registerSecurityFeature();
    registerSecurityFeature();
    // One Stop All command (core `ai.stopAll`), which opens the security confirm dialog.
    expect(commands.filter((c) => c.title === "Stop All AI")).toHaveLength(1);
    expect(commands.find((c) => c.id === "ai.stopAll")?.shortcut).toBe("Ctrl+Shift+Backspace");
    expect(commands.some((c) => c.id === "security.openApprovals")).toBe(true);
    expect(settingsSections.map((s) => s.id)).toEqual(expect.arrayContaining(["security", "secrets"]));
    commands.find((c) => c.id === "security.openApprovals")?.run({ projectId: null });
    expect(useSecurityStore.getState().inboxOpen).toBe(true);
  });
});
