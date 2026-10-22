/**
 * Dashboard Builder panel (ADR-0034); logic in `features/dashboards`. **Owner: WP3.**
 */
import { DashboardBuilder } from "../../features/dashboards";
import type { PanelProps } from "../types";

export default function DashboardsPanel(_props: PanelProps) {
  return <DashboardBuilder />;
}
