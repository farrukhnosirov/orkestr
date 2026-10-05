/** Robot Designer and robot window UI (English and Russian). */
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { RobotDesigner } from "./designer/RobotDesigner";
import { defaultDesign } from "./model/design";
import { robotMenu } from "./model/menu";
import { RobotMenu } from "./window/RobotMenu";
import { SpeechBubble } from "./window/SpeechBubble";

afterEach(async () => {
  cleanup();
  await applyLanguagePreference("en");
});

describe("RobotDesigner", () => {
  it("edits a design and saves a normalized copy", async () => {
    await applyLanguagePreference("en");
    const onSave = vi.fn();
    render(<RobotDesigner value={defaultDesign()} mode="Balanced" saving={false} onSave={onSave} />);
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toHaveProperty("disabled", true);
    fireEvent.click(screen.getByRole("radio", { name: "Ember" }));
    fireEvent.click(screen.getByRole("button", { name: "Halo" }));
    expect(save).toHaveProperty("disabled", false);
    fireEvent.click(save);
    expect(onSave).toHaveBeenCalledTimes(1);
    const saved = onSave.mock.calls[0]?.[0] as ReturnType<typeof defaultDesign>;
    expect(saved.skin).toBe("ember");
    expect(saved.accessories).toEqual(["antenna", "halo"]);
  });

  it("explains that 3D needs Ultra Visual (Russian)", async () => {
    await applyLanguagePreference("ru");
    render(<RobotDesigner value={{ ...defaultDesign(), mode: "ThreeD" }} mode="Balanced" saving={false} onSave={() => undefined} />);
    expect(screen.getByText(/только в режиме производительности «Максимум»/)).toBeTruthy();
    expect(screen.getByRole("radio", { name: "Туманность" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeTruthy();
  });
});

describe("robot window parts", () => {
  it("shows the Scenario H bubble in Russian with the right plural", async () => {
    await applyLanguagePreference("ru");
    render(
      <SpeechBubble
        bubble={{ key: "testsFailed", values: { count: 3 }, tone: "danger", ttlMs: 0, important: true, chatty: false }}
        nonce={1}
        reducedMotion
      />,
    );
    expect(screen.getByText("Не прошли 3 теста")).toBeTruthy();
  });

  it("context menu: Stop agent, project submenu, Escape closes", async () => {
    await applyLanguagePreference("en");
    const onAction = vi.fn();
    const onAssign = vi.fn();
    const onClose = vi.fn();
    const items = robotMenu({ state: "Coding", stoppedAt: null, voiceEnabled: false, assignableProjects: 1, hasTemplate: true });
    const project = {
      id: "0190f0b0-0000-7000-8000-000000000009",
      name: "Schoolify",
      color: "#22d3ee",
    } as Parameters<typeof RobotMenu>[0]["projects"][number];
    render(<RobotMenu items={items} projects={[project]} onAction={onAction} onAssign={onAssign} onClose={onClose} />);
    fireEvent.click(screen.getByRole("menuitem", { name: "Stop agent" }));
    expect(onAction).toHaveBeenCalledWith("stop");
    fireEvent.click(screen.getByRole("menuitem", { name: "Assign to project…" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Schoolify" }));
    expect(onAssign).toHaveBeenCalledWith(project.id);
    act(() => {
      fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    });
    expect(onClose).not.toHaveBeenCalled(); // first Escape leaves the submenu
    act(() => {
      fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    });
    expect(onClose).toHaveBeenCalled();
  });
});
