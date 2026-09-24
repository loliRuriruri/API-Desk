import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Sidebar, type PageKey } from "./components/Sidebar";
import { SearchPalette } from "./components/SearchPalette";
import { ToastProvider, useToast } from "./components/ToastProvider";
import { ApisPage } from "./pages/ApisPage";
import { DashboardPage } from "./pages/DashboardPage";
import { ImportPage } from "./pages/ImportPage";
import { LocalServicesPage } from "./pages/LocalServicesPage";
import { ModelsPage } from "./pages/ModelsPage";
import { ProjectDetailPage } from "./pages/ProjectDetailPage";
import { ProjectsPage } from "./pages/ProjectsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { VaultGate } from "./pages/VaultGate";
import { MiniApp } from "./MiniApp";
import {
  listCredentialsWithContext,
  listMonitorSnapshots,
  listUsageSnapshots,
} from "./lib/db/repo";
import { errorMessage } from "./lib/errors";
import {
  applyTheme,
  DEFAULT_SETTINGS,
  loadAlertThresholds,
  loadMonitorCaps,
  loadSettings,
  type AppSettings,
} from "./lib/settings";
import { vaultLock, vaultStatus, type VaultStatus } from "./lib/vault";
import {
  launchedFromAutostart,
  notifyDesktop,
  notifyVaultChanged,
  onMainNavigate,
  onSettingsChanged,
  onUsageRefresh,
  openMiniWindow,
  setTrayResident,
} from "./lib/desktop";
import {
  collectAlerts,
  credentialRemainingPercent,
  monitorRemainingPercent,
} from "./lib/alerts";
import { autostartLocalServices } from "./lib/localServices";
import { refreshStaleUsageSnapshots } from "./lib/usageRefresh";
import type { SearchResult } from "./types/domain";

interface Route {
  page: PageKey;
  focusKind?: string;
  focusId?: string;
  nonce: number;
}

const PAGE_TITLES: Record<PageKey, string> = {
  dashboard: "대시보드",
  apis: "API 관리",
  models: "모델",
  services: "로컬 서비스",
  projects: "프로젝트",
  settings: "설정",
  import: "텍스트 가져오기",
};

function VaultLockButton({ onLocked }: { onLocked: () => void }) {
  const { notify } = useToast();
  const [busy, setBusy] = useState(false);
  const lock = async () => {
    setBusy(true);
    try {
      await vaultLock();
      onLocked();
    } catch (error) {
      notify(errorMessage(error), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <button type="button" className="button button-small" disabled={busy} onClick={() => void lock()}>
      {busy ? "잠그는 중…" : "잠그기"}
    </button>
  );
}

export default function App() {
  const isMini = new URLSearchParams(window.location.search).get("window") === "mini";
  if (isMini) {
    return (
      <ToastProvider>
        <MiniApp />
      </ToastProvider>
    );
  }
  return <MainApp />;
}

function MainApp() {
  const [vault, setVault] = useState<VaultStatus | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [route, setRoute] = useState<Route>({ page: "dashboard", nonce: 0 });
  const [searchOpen, setSearchOpen] = useState(false);
  const [refreshNonce, setRefreshNonce] = useState(0);
  const alertedRef = useRef<Set<string>>(new Set());
  const miniOpenedRef = useRef(false);
  const usageRefreshedRef = useRef(false);
  const localServicesStartedRef = useRef(false);

  useEffect(() => {
    void notifyVaultChanged();
  }, [vault?.state]);

  useEffect(() => {
    if (vault?.state !== "unlocked" || usageRefreshedRef.current) return;
    usageRefreshedRef.current = true;
    void (async () => {
      try {
        const updated = await refreshStaleUsageSnapshots(6 * 60 * 60 * 1000);
        if (updated > 0) {
          setRefreshNonce((current) => current + 1);
          setRoute((current) => ({ ...current, nonce: Date.now() }));
        }
      } catch {
        // automatic usage refresh is best-effort
      }
    })();
  }, [vault?.state]);

  useEffect(() => {
    if (!settings.openMiniOnStart || miniOpenedRef.current) return;
    miniOpenedRef.current = true;
    // Creating a second webview while the main window is still loading leaves
    // the mini window blank, so defer it a moment.
    const timer = window.setTimeout(() => {
      void (async () => {
        if (await launchedFromAutostart()) return;
        await openMiniWindow();
      })();
    }, 1500);
    return () => window.clearTimeout(timer);
  }, [settings.openMiniOnStart]);

  const refreshVault = useCallback(async () => {
    try {
      const status = await vaultStatus();
      setVault(status);
      setBootError(null);
    } catch (error) {
      setBootError(errorMessage(error));
    }
  }, []);

  useEffect(() => {
    let active = true;
    vaultStatus()
      .then((status) => {
        if (!active) return;
        setVault(status);
        setBootError(null);
      })
      .catch((error: unknown) => {
        if (active) setBootError(errorMessage(error));
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    loadSettings()
      .then((value) => {
        if (active) setSettings(value);
      })
      .catch(() => {
        if (active) setSettings(DEFAULT_SETTINGS);
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    applyTheme(settings.theme);
  }, [settings.theme]);

  useEffect(() => {
    void setTrayResident(settings.trayResident);
  }, [settings.trayResident]);

  useEffect(() => {
    if (vault?.state !== "unlocked" || settings.autoLockMinutes <= 0) return;
    const timeoutMs = settings.autoLockMinutes * 60_000;
    let timer = window.setTimeout(() => {
      void vaultLock().then(() => refreshVault());
    }, timeoutMs);
    const reset = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        void vaultLock().then(() => refreshVault());
      }, timeoutMs);
    };
    const events: Array<keyof WindowEventMap> = ["mousemove", "mousedown", "keydown", "wheel"];
    events.forEach((event) => window.addEventListener(event, reset));
    return () => {
      window.clearTimeout(timer);
      events.forEach((event) => window.removeEventListener(event, reset));
    };
  }, [vault?.state, settings.autoLockMinutes, refreshVault]);

  useEffect(
    () =>
      onUsageRefresh(() => {
        setRefreshNonce((current) => current + 1);
      }),
    [],
  );

  // 로컬 서비스 자동 실행: Vault 잠금 여부와 무관하게 시도한다.
  // (키를 저장해 둔 서비스는 백엔드가 잠금 상태에서 차단하고, 무인증 모드는 실행된다.)
  useEffect(() => {
    if (localServicesStartedRef.current) return;
    localServicesStartedRef.current = true;
    void autostartLocalServices().catch(() => {});
  }, []);

  useEffect(
    () =>
      onMainNavigate((page) => {
        const known: PageKey[] = [
          "dashboard",
          "apis",
          "models",
          "projects",
          "settings",
          "import",
        ];
        if (!known.includes(page as PageKey)) return;
        setRoute({ page: page as PageKey, nonce: Date.now() });
      }),
    [],
  );

  useEffect(
    () =>
      onSettingsChanged(() => {
        void loadSettings()
          .then((value) => setSettings(value))
          .catch(() => {});
      }),
    [],
  );

  useEffect(() => {
    if (!settings.alertsEnabled) return;
    let active = true;
    Promise.all([
      listUsageSnapshots(),
      listMonitorSnapshots(),
      listCredentialsWithContext(),
      loadAlertThresholds(),
      loadMonitorCaps(),
    ])
      .then(([usageSnapshots, monitorSnapshots, credentials, thresholds, caps]) => {
        if (!active) return;
        const credentialById = new Map(credentials.map((item) => [item.id, item]));
        const candidates = [
          ...usageSnapshots.map((snapshot) => ({
            key: `usage-${snapshot.credential_id}`,
            label: credentialById.get(snapshot.credential_id)?.name ?? snapshot.adapter,
            remainingPercent: credentialRemainingPercent(
              snapshot.remaining,
              snapshot.limit_total,
            ),
            thresholdPercent: thresholds[`usage-${snapshot.credential_id}`] ?? null,
          })),
          ...monitorSnapshots.map((snapshot) => ({
            key: `monitor-${snapshot.monitor}`,
            label:
              snapshot.monitor === "codex"
                ? "Codex"
                : snapshot.monitor === "grok"
                  ? "Grok"
                  : "Antigravity",
            remainingPercent: monitorRemainingPercent(
              snapshot.monitor,
              snapshot.details_json,
              caps[snapshot.monitor] ?? null,
            ),
            thresholdPercent: thresholds[`monitor-${snapshot.monitor}`] ?? null,
          })),
        ];
        for (const alert of collectAlerts(candidates, settings.alertThresholdPercent)) {
          const dedupeKey = `${alert.key}:${Math.round(alert.remainingPercent)}`;
          if (alertedRef.current.has(dedupeKey)) continue;
          alertedRef.current.add(dedupeKey);
          void notifyDesktop("API Desk · 사용량 경고", alert.message);
        }
      })
      .catch(() => {
        // alerts are best-effort
      });
    return () => {
      active = false;
    };
  }, [
    settings.alertsEnabled,
    settings.alertThresholdPercent,
    refreshNonce,
  ]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setSearchOpen(true);
      }
      if (event.key === "Escape") {
        setSearchOpen(false);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const navigate = useCallback((page: PageKey, focusKind?: string, focusId?: string) => {
    setRoute({ page, focusKind, focusId, nonce: Date.now() });
    setSearchOpen(false);
  }, []);

  const handleSearchSelect = useCallback(
    (result: SearchResult) => {
      switch (result.kind) {
        case "provider":
        case "account":
        case "credential":
          navigate("apis", result.kind, result.id);
          break;
        case "model":
          navigate("models", result.kind, result.id);
          break;
        case "project":
          navigate("projects", "project-detail", result.id);
          break;
        default:
          navigate("apis");
      }
    },
    [navigate],
  );

  const apisFocus = useMemo(
    () =>
      route.page === "apis" && route.focusId && route.focusKind
        ? { kind: route.focusKind, id: route.focusId }
        : null,
    [route],
  );

  const renderPage = () => {
    switch (route.page) {
      case "dashboard":
        return (
          <DashboardPage
            key={`dashboard-${refreshNonce}`}
            onOpenApis={() => navigate("apis")}
          />
        );
      case "apis":
        return (
          <ApisPage
            key={`apis-${route.nonce}`}
            focus={apisFocus}
            autoHideSecs={settings.autoHideSecs}
            clipboardClearSecs={settings.clipboardClearSecs}
          />
        );
      case "models":
        return <ModelsPage key={`models-${route.nonce}`} />;
      case "services":
        return <LocalServicesPage key={`services-${route.nonce}`} />;
      case "projects":
        if (route.focusKind === "project-detail" && route.focusId) {
          return (
            <ProjectDetailPage
              key={`project-${route.focusId}`}
              projectId={route.focusId}
              onBack={() => navigate("projects")}
              onDeleted={() => navigate("projects")}
            />
          );
        }
        return (
          <ProjectsPage
            key={`projects-${route.nonce}`}
            focusId={route.focusKind === "project" ? (route.focusId ?? null) : null}
            defaultProjectDir={settings.defaultProjectDir}
            onOpenProject={(projectId) => navigate("projects", "project-detail", projectId)}
          />
        );
      case "settings":
        return (
          <SettingsPage
            settings={settings}
            onChange={setSettings}
            onVaultChanged={() => void refreshVault()}
          />
        );
      case "import":
        return <ImportPage />;
      default:
        return null;
    }
  };

  if (bootError) {
    return (
      <div className="gate">
        <div className="gate-card">
          <h1>API Desk를 시작할 수 없습니다</h1>
          <p className="gate-error">{bootError}</p>
        </div>
      </div>
    );
  }

  if (!vault) {
    return <div className="gate splash">Vault 확인 중…</div>;
  }

  if (vault.state !== "unlocked") {
    return <VaultGate status={vault} onUnlocked={() => void refreshVault()} />;
  }

  return (
    <ToastProvider>
      <div className="app">
        <Sidebar
          page={route.page}
          onNavigate={(page) => navigate(page)}
          onLockVault={() => void refreshVault()}
          locked={false}
        />
        <div className="main">
          <header className="topbar">
            <h1>{PAGE_TITLES[route.page]}</h1>
            <div className="topbar-actions">
              <button
                type="button"
                className="search-trigger"
                onClick={() => setSearchOpen(true)}
              >
                프로바이더 · 키 · 모델 검색… <kbd>Ctrl K</kbd>
              </button>
              <button
                type="button"
                className="button button-small"
                onClick={() => navigate("import")}
              >
                텍스트 가져오기
              </button>
              <button
                type="button"
                className="button button-small"
                onClick={() => void openMiniWindow()}
                title="항상 위에 표시되는 미니 사용량 창을 엽니다"
              >
                미니 창
              </button>
              <span className="vault-status" title="Vault가 열려 있어 키를 읽고 저장할 수 있습니다">
                <span className="vault-dot" />
                Vault 열림
              </span>
              <VaultLockButton onLocked={() => void refreshVault()} />
            </div>
          </header>
          <main className="content">{renderPage()}</main>
        </div>
        {searchOpen ? (
          <SearchPalette onClose={() => setSearchOpen(false)} onSelect={handleSearchSelect} />
        ) : null}
      </div>
    </ToastProvider>
  );
}
