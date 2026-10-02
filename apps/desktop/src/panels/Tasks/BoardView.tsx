import type { Task, TaskStatus } from "@orkestr/protocol";
import { clsx } from "clsx";
import { type DragEvent, useState } from "react";

import { errorMessage, taskActions } from "./data";
import { BOARD_COLUMNS, type BoardColumn, dropTarget, shortId, sortTasks, statusColor } from "./model";
import { PriorityChip, StatusBadge } from "./parts";
import { useTasksUi } from "./store";

const MIME = "application/x-orkestr-task";

/** Kanban by status group. Dragging a card onto a column performs the first valid transition. */
export function BoardView({ tasks }: { tasks: Task[] }) {
  const select = useTasksUi((s) => s.select);
  const selected = useTasksUi((s) => s.selectedTaskId);
  const toast = useTasksUi((s) => s.toast);
  const [dragging, setDragging] = useState<Task | null>(null);
  const [over, setOver] = useState<string | null>(null);
  const sorted = sortTasks(tasks, "priority", "desc");

  const onDrop = async (e: DragEvent, column: BoardColumn) => {
    e.preventDefault();
    setOver(null);
    const id = e.dataTransfer.getData(MIME);
    const task = tasks.find((t) => t.id === id);
    setDragging(null);
    if (!task) return;
    const to = dropTarget(task.status, column);
    if (!to) {
      if (!column.statuses.includes(task.status)) toast("danger", `${task.status} → ${column.title} is not an allowed transition.`);
      return;
    }
    try {
      await taskActions.transition(task.id, to);
    } catch (err) {
      toast("danger", errorMessage(err));
    }
  };

  return (
    <div className="tasks-board" role="region" aria-label="Task board">
      {BOARD_COLUMNS.map((column) => {
        const cards = sorted.filter((t) => column.statuses.includes(t.status));
        const target: TaskStatus | null = dragging ? dropTarget(dragging.status, column) : null;
        return (
          <section
            key={column.id}
            className={clsx(
              "tasks-col",
              dragging && (target ? "tasks-col--droppable" : "tasks-col--blocked"),
              over === column.id && target && "tasks-col--over",
            )}
            aria-label={`${column.title}, ${cards.length} tasks`}
            onDragOver={(e) => {
              if (target) {
                e.preventDefault();
                e.dataTransfer.dropEffect = "move";
                if (over !== column.id) setOver(column.id);
              }
            }}
            onDragLeave={() => over === column.id && setOver(null)}
            onDrop={(e) => void onDrop(e, column)}
          >
            <header className="tasks-col__header">
              <span>{column.title}</span>
              <span className="tasks-col__count">{cards.length}</span>
              {target && <span className="tasks-col__target">→ {target}</span>}
            </header>
            <div className="tasks-col__cards">
              {cards.length === 0 ? (
                <div className="tasks-col__empty">No tasks</div>
              ) : (
                cards.map((t) => (
                  <button
                    key={t.id}
                    type="button"
                    draggable
                    className={clsx("tasks-card", selected === t.id && "tasks-card--selected")}
                    style={{ ["--status-color" as string]: statusColor(t.status) }}
                    onDragStart={(e) => {
                      e.dataTransfer.setData(MIME, t.id);
                      e.dataTransfer.effectAllowed = "move";
                      setDragging(t);
                    }}
                    onDragEnd={() => {
                      setDragging(null);
                      setOver(null);
                    }}
                    onClick={() => select(t.id)}
                    aria-label={`${t.title}, ${t.status}, ${t.priority} priority`}
                  >
                    <span className="tasks-card__title">{t.title}</span>
                    <span className="tasks-card__meta">
                      <StatusBadge status={t.status} />
                      <PriorityChip priority={t.priority} compact />
                      <span className="tasks-mono tasks-card__id">#{shortId(t.id)}</span>
                    </span>
                  </button>
                ))
              )}
            </div>
          </section>
        );
      })}
    </div>
  );
}
