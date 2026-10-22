import type { DeviceRecord } from "@orkestr/client";
import { Button, Spinner } from "@orkestr/ui";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { useDeps } from "./deps";
import type { AuthState } from "./gateway";
import type { RemoteClient } from "./remote";
import { Login } from "./screens/Login";
import { Pairing } from "./screens/Pairing";
import { Workspace } from "./screens/Workspace";

type View =
  | { kind: "loading" }
  | { kind: "offline" }
  | { kind: "login"; state: AuthState }
  | { kind: "pairing" }
  | { kind: "connecting"; device: DeviceRecord }
  | { kind: "connectFailed"; device: DeviceRecord }
  | { kind: "workspace"; device: DeviceRecord; client: RemoteClient };

/** Web client shell: login → pairing (first time) → E2E connection → workspace. */
export function App() {
  const { t } = useTranslation();
  const deps = useDeps();
  const [view, setView] = useState<View>({ kind: "loading" });
  const [token, setToken] = useState<string | undefined>(undefined);

  const connect = useCallback(
    async (device: DeviceRecord, sessionToken = token) => {
      setView({ kind: "connecting", device });
      try {
        setView({ kind: "workspace", device, client: await deps.connect(device, sessionToken) });
      } catch {
        setView({ kind: "connectFailed", device });
      }
    },
    [deps, token],
  );

  const afterLogin = useCallback(
    async (sessionToken?: string) => {
      const devices = await deps.store.list();
      const device = devices.find((d) => d.gatewayUrl.replace(/\/+$/, "") === deps.gateway.url.replace(/\/+$/, "")) ?? devices[0];
      if (device) await connect(device, sessionToken);
      else setView({ kind: "pairing" });
    },
    [deps, connect],
  );

  useEffect(() => {
    deps.gateway.state().then(
      (state) => (state.authenticated ? void afterLogin() : setView({ kind: "login", state })),
      () => setView({ kind: "offline" }),
    );
    // Run once on mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  switch (view.kind) {
    case "loading":
    case "connecting":
      return (
        <main className="web-center">
          <Spinner label={view.kind === "loading" ? t("common.loading") : t("app.connecting")} />
        </main>
      );
    case "offline":
      return (
        <main className="web-center">
          <p role="alert">{t("app.offline")}</p>
        </main>
      );
    case "login":
      return (
        <Login
          state={view.state}
          onLoggedIn={(r) => {
            setToken(r.token);
            void afterLogin(r.token);
          }}
        />
      );
    case "pairing":
      return <Pairing onPaired={(d) => void connect(d)} />;
    case "connectFailed":
      return (
        <main className="web-center web-stack">
          <p role="alert">{t("app.connectFailed")}</p>
          <Button onClick={() => void connect(view.device)}>{t("common.retry")}</Button>
          <Button variant="ghost" onClick={() => void deps.store.delete(view.device.id).then(() => setView({ kind: "pairing" }))}>
            {t("settings.forget")}
          </Button>
        </main>
      );
    case "workspace":
      return (
        <Workspace
          client={view.client}
          device={view.device}
          onLogout={() => {
            view.client.close();
            setView({ kind: "loading" });
            void deps.gateway
              .logout()
              .catch(() => undefined)
              .then(() => deps.gateway.state())
              .then(
                (state) => setView({ kind: "login", state }),
                () => setView({ kind: "offline" }),
              );
          }}
          onForget={() => {
            view.client.close();
            void deps.store.delete(view.device.id).then(() => setView({ kind: "pairing" }));
          }}
        />
      );
  }
}
