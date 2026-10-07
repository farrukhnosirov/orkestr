// Claude Code permission card (responses, keys, diff preview, risk, resolved
// state), mode switcher (bypass confirmation) and Russian rendering.
import type { ClaudePermissionMode } from "@orkestr/protocol";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PermissionItem } from "../../../features/claudeCode/model";
import { i18n } from "../../../i18n";
import { ModeSwitcher, PermissionCard } from "./index";

const ROOT = "C:/work/proj";

function item(over: Partial<PermissionItem> = {}): PermissionItem {
  return {
    kind: "permission",
    id: "req-1",
    toolName: "Bash",
    toolUseId: "toolu_1",
    input: { command: "npm test", description: "Run tests" },
    description: null,
    blockedPath: null,
    risk: null,
    canRemember: true,
    status: "pending",
    byPolicy: false,
    message: null,
    ...over,
  };
}

function renderCard(request: PermissionItem) {
  const onRespond = vi.fn();
  render(<PermissionCard request={request} root={ROOT} cwd={ROOT} onRespond={onRespond} />);
  return { onRespond, card: screen.getByRole("group") };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

describe("PermissionCard", () => {
  it("shows a Bash command and answers with the buttons", () => {
    const { onRespond, card } = renderCard(item());
    expect(card.getAttribute("aria-label")).toBe("Claude wants to use Bash");
    expect(screen.getByText("Run a command")).toBeTruthy();
    expect(screen.getByText("npm test").tagName).toBe("PRE");
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    expect(onRespond).toHaveBeenLastCalledWith("allowOnce");
    fireEvent.click(screen.getByRole("button", { name: "Allow for this session" }));
    expect(onRespond).toHaveBeenLastCalledWith("allowSession");
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    expect(onRespond).toHaveBeenLastCalledWith("deny");
  });

  it("Enter allows once, Esc denies", () => {
    const { onRespond, card } = renderCard(item());
    fireEvent.keyDown(card, { key: "Enter" });
    expect(onRespond).toHaveBeenLastCalledWith("allowOnce");
    fireEvent.keyDown(card, { key: "Escape" });
    expect(onRespond).toHaveBeenLastCalledWith("deny");
    expect(onRespond).toHaveBeenCalledTimes(2);
  });

  it("denies with a reason", () => {
    const { onRespond } = renderCard(item());
    fireEvent.click(screen.getByRole("button", { name: "Deny with a reason…" }));
    const box = screen.getByRole("textbox", { name: "Reason for Claude" });
    fireEvent.change(box, { target: { value: "use rg instead" } });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(onRespond).toHaveBeenCalledWith("deny", "use rg instead");
  });

  it("Esc in the reason form closes it without answering", () => {
    const { onRespond } = renderCard(item());
    fireEvent.click(screen.getByRole("button", { name: "Deny with a reason…" }));
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Reason for Claude" }), { key: "Escape" });
    expect(onRespond).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Allow once" })).toBeTruthy();
  });

  it("shows the path and a mini diff for Edit", () => {
    renderCard(
      item({
        toolName: "Edit",
        input: { file_path: `${ROOT}/src/a.ts`, old_string: "const a = 1;\nkeep", new_string: "const a = 2;\nkeep" },
      }),
    );
    expect(screen.getByText("Edit a file")).toBeTruthy();
    expect(screen.getByText("src/a.ts")).toBeTruthy();
    const diff = screen.getByRole("figure");
    expect(diff.textContent).toContain("-const a = 1;");
    expect(diff.textContent).toContain("+const a = 2;");
    expect(diff.textContent).not.toContain("keep");
  });

  it("caps long Write previews", () => {
    const content = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
    renderCard(item({ toolName: "Write", input: { file_path: "/elsewhere/x.txt", content } }));
    expect(screen.getByText(/outside the project/)).toBeTruthy();
    expect(screen.getByText("…and 8 more lines")).toBeTruthy();
  });

  it("highlights dangerous commands and offers no session-wide allow", () => {
    const { card } = renderCard(
      item({
        input: { command: "rm -rf build" },
        risk: { level: "dangerous", category: "RecursiveDelete", reason: "recursive delete" },
      }),
    );
    expect(card.className).toContain("ccx-perm--dangerous");
    expect(screen.getByText(/Dangerous · Recursive delete/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Allow for this session" })).toBeNull();
  });

  it("shows the outcome once answered", () => {
    const { onRespond, card } = renderCard(item({ status: "allowSession" }));
    expect(screen.getByRole("status").textContent).toBe("Allowed for this session");
    expect(screen.queryByRole("button")).toBeNull();
    fireEvent.keyDown(card, { key: "Enter" });
    expect(onRespond).not.toHaveBeenCalled();
    cleanup();
    renderCard(item({ status: "deny", byPolicy: true, message: "forbidden" }));
    expect(screen.getByRole("status").textContent).toBe("Denied by Orkestr policy — forbidden");
  });

  it("renders in Russian", async () => {
    await i18n.changeLanguage("ru");
    renderCard(item());
    expect(screen.getByRole("group").getAttribute("aria-label")).toBe("Claude хочет использовать Bash");
    expect(screen.getByRole("button", { name: "Разрешить один раз" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Разрешить в этом сеансе" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Запретить" })).toBeTruthy();
    expect(screen.getByText("Enter — разрешить один раз · Esc — запретить")).toBeTruthy();
  });
});

describe("ModeSwitcher", () => {
  function renderSwitcher(mode: ClaudePermissionMode = "default") {
    const onChange = vi.fn();
    render(<ModeSwitcher mode={mode} onChange={onChange} />);
    return onChange;
  }

  it("switches safe modes directly", () => {
    const onChange = renderSwitcher();
    expect(screen.getByRole("radio", { name: "Default" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByRole("radio", { name: "Plan" }));
    expect(onChange).toHaveBeenCalledWith("plan");
  });

  it("asks for confirmation before bypassing permissions", () => {
    const onChange = renderSwitcher();
    fireEvent.click(screen.getByRole("radio", { name: "No confirmations" }));
    expect(onChange).not.toHaveBeenCalled();
    const dialog = screen.getByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();

    fireEvent.click(screen.getByRole("radio", { name: "No confirmations" }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Turn off confirmations" }));
    expect(onChange).toHaveBeenCalledWith("bypassPermissions", true);
  });

  it("renders in Russian", async () => {
    await i18n.changeLanguage("ru");
    renderSwitcher("acceptEdits");
    expect(screen.getByRole("radiogroup", { name: "Режим разрешений" })).toBeTruthy();
    expect(screen.getByRole("radio", { name: "Принимать правки" }).getAttribute("aria-checked")).toBe("true");
    for (const name of ["По умолчанию", "План", "Без подтверждений"]) expect(screen.getByRole("radio", { name })).toBeTruthy();
  });
});
