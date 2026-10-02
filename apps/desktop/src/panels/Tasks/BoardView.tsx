import type { Task, TaskStatus } from "@orkestr/protocol";
import { clsx } from "clsx";
import { type DragEvent, useState } from "react";

import { useT } from "../../i18n";
import { errorMessage, taskActions } from "./data";
import { BOARD_COLUMNS, type BoardColumn, dropTarget, shortId, sortTasks, statusColor } from "./model";
import { PriorityChip, StatusBadge } from "./parts";
import { useTasksUi } from "./store";

const MIME = "application/x-orkestr-task";

/** Kanban by status group. Dragging a card onto a column performs the first valid transition. */
export function BoardView({ tasks }: { tasks: Task[] }) {
  const { t } = useT(["tasks", "common"]);
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
    const task = tasks.find((x) => x.id === id);
    setDragging(null);
    if (!task) return;
    const to = dropTarget(task.status, column);
    if (!to) {
      if (!column.statuses.includes(task.status)) {
        toast("danger", t("board.notAllowed", { from: t(`common:status.task.${task.status}`), to: t(`board.columns.${column.id}`) }));
      }
      return;
    }
    try {
      await taskActions.transition(task.id, to);
    } catch (err) {
      toast("danger", errorMessage(err));
    }
  };

  return (
    <div className="tasks-board" role="region" aria-label={t("board.label")}>
      {BOARD_COLUMNS.map((column) => {
        const cards = sorted.filter((x) => column.statuses.includes(x.status));
        const title = t(`board.columns.${column.id}`);
        const target: TaskStatus | null = dragging ? dropTarget(dragging.status, column) : null;
        return (
          <section
            key={column.id}
            className={clsx(
              "tasks-col",
              dragging && (target ? "tasks-col--droppable" : "tasks-col--blocked"),
              over === column.id && target && "tasks-col--over",
            )}
            aria-label={t("board.column", { title, tasks: t("common:count.tasks", { count: cards.length }) })}
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
              <span>{title}</span>
              <span className="tasks-col__count">{cards.length}</span>
              {target && <span className="tasks-col__target">→ {t(`common:status.task.${target}`)}</span>}
            </header>
            <div className="tasks-col__cards">
              {cards.length === 0 ? (
                <div className="tasks-col__empty">{t("board.noTasks")}</div>
              ) : (
                cards.map((card) => (
                  <button
                    key={card.id}
                    type="button"
                    draggable
                    className={clsx("tasks-card", selected === card.id && "tasks-card--selected")}
                    style={{ ["--status-color" as string]: statusColor(card.status) }}
                    onDragStart={(e) => {
                      e.dataTransfer.setData(MIME, card.id);
                      e.dataTransfer.effectAllowed = "move";
                      setDragging(card);
                    }}
                    onDragEnd={() => {
                      setDragging(null);
                      setOver(null);
                    }}
                    onClick={() => select(card.id)}
                    aria-label={t("board.card", {
                      title: card.title,
                      status: t(`common:status.task.${card.status}`),
                      priority: t("priorityTitle", { priority: t(`priority.${card.priority}`) }),
                    })}
                  >
                    <span className="tasks-card__title">{card.title}</span>
                    <span className="tasks-card__meta">
                      <StatusBadge status={card.status} />
                      <PriorityChip priority={card.priority} compact />
                      <span className="tasks-mono tasks-card__id">#{shortId(card.id)}</span>
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
