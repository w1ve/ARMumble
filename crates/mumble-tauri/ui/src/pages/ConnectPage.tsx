import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore, setExplicitRoleForServer } from "../store";
import {
  getSavedServers,
  addServer,
  removeServer,
  updateServer,
  getServerPassword,
  setServerPassword,
} from "../serverStorage";
import { getPreferences, updatePreferences } from "../preferencesStorage";
import type { SavedServer, ServerPingResult, UserMode, ClientRole } from "../types";
import { RR_PORT, RR_PASSWORD, type BuiltinServer } from "../builtinServers";
import { notifyServersChanged } from "../stationIdentity";
import ServerList from "../components/server/ServerList";
import BuiltinServerList from "../components/server/BuiltinServerList";
import ServerEditSheet from "../components/server/ServerEditSheet";
import BrandLogo from "../components/elements/BrandLogo";
import PasswordDialog from "../components/server/PasswordDialog";
import { InfoFilledIcon, CheckIcon } from "../icons";
import { TID } from "../testids";
import styles from "./ConnectPage.module.css";

type View = "loading" | "servers" | "wizard";

/** Module-level cache: "host:port" -> epoch-ms of last ping invocation. */
const pingCache = new Map<string, number>();

interface StepDef {
  readonly title: string;
  readonly subtitle: string;
  readonly hint: string;
}

export default function ConnectPage() {
  const { t } = useTranslation(["server", "common"]);
  const {
    connect, disconnect, status, error, passwordRequired, pendingConnect,
    retryWithPassword, dismissPasswordPrompt, bootstrapStage,
  } = useAppStore();
  // Keep the loading bar up not just while the TLS handshake runs, but
  // through the post-connect data bootstrap (channels, users, own session,
  // server info).  Otherwise the user would land on an empty chat view.
  const isConnecting = status === "connecting" || bootstrapStage !== null;

  /* -- which server card is actively connecting ------------------- */
  const [connectingServerId, setConnectingServerId] = useState<string | null>(null);

  // Clear the connecting indicator when we leave the "connecting" state
  useEffect(() => {
    if (!isConnecting) setConnectingServerId(null);
  }, [isConnecting]);

  /* -- user mode ------------------------------------------------- */
  const [userMode, setUserMode] = useState<UserMode>("normal");
  const [defaultUsername, setDefaultUsername] = useState("");
  const [defaultRole, setDefaultRole] = useState<ClientRole>("operator");
  const tStr = t as (key: string) => string;
  const STEPS = useMemo<StepDef[]>(() => {
    const mode = userMode === "normal" ? "normal" : "expert";
    const count = userMode === "normal" ? 2 : 3;
    return Array.from({ length: count }, (_, i) => ({
      title: tStr(`wizard.${mode}.step${i}.title`),
      subtitle: tStr(`wizard.${mode}.step${i}.subtitle`),
      hint: tStr(`wizard.${mode}.step${i}.hint`),
    }));
  }, [userMode, tStr]);

  /* -- saved servers --------------------------------------------- */
  const [savedServers, setSavedServers] = useState<SavedServer[]>([]);
  const [view, setView] = useState<View>("loading");

  /* -- ping results keyed by server id --------------------------- */
  const [pings, setPings] = useState<Record<string, ServerPingResult>>({});

  /**
   * Ping a list of servers, throttled to at most once per 60 s per
   * host:port.  This prevents the Mumble server from seeing a flood of
   * raw TCP connections (which it logs as ban rejections for banned IPs).
   */
  const pingServers = useCallback((servers: SavedServer[]) => {
    const THROTTLE_MS = 60_000;
    const now = Date.now();

    for (const s of servers) {
      const key = `${s.host}:${s.port}`;
      const last = pingCache.get(key);
      if (last !== undefined && now - last < THROTTLE_MS) {
        // Still fresh - re-use cached result already in state.
        continue;
      }
      pingCache.set(key, now);

      invoke<ServerPingResult>("ping_server", { host: s.host, port: s.port })
        .then((result) =>
          setPings((prev) => ({ ...prev, [s.id]: result })),
        )
        .catch(() =>
          setPings((prev) => ({
            ...prev,
            [s.id]: { online: false, latency_ms: null, user_count: null, max_user_count: null, server_version: null },
          })),
        );
    }
  }, []);

  useEffect(() => {
    Promise.all([getSavedServers(), getPreferences()]).then(([list, prefs]) => {
      setUserMode(prefs.userMode);
      setDefaultUsername(prefs.defaultUsername);
      setUsername(prefs.defaultUsername);
      setDefaultRole(prefs.defaultRole ?? "operator");
      setRole(prefs.defaultRole ?? "operator");
      setSavedServers(list);
      // Always land on the server list: even with no saved servers the
      // built-in Remote Radio network is shown there, so the wizard is only
      // reached by explicitly choosing "Add Server".
      setView("servers");
      if (list.length > 0) pingServers(list);
    });
  }, [pingServers]);

  /* -- certificate state ----------------------------------------- */
  const [availableCerts, setAvailableCerts] = useState<string[]>([]);
  const [certLabel, setCertLabel] = useState<string>("default");
  const [creatingCert, setCreatingCert] = useState(false);
  const [newCertName, setNewCertName] = useState("");

  const refreshCerts = () =>
    invoke<string[]>("list_certificates")
      .then(setAvailableCerts)
      .catch(() => setAvailableCerts([]));

  useEffect(() => {
    refreshCerts();
  }, []);

  const handleCreateCert = async () => {
    const name = newCertName.trim();
    if (!name) return;
    await invoke("generate_certificate", { label: name });
    await refreshCerts();
    setCertLabel(name);
    setNewCertName("");
    setCreatingCert(false);
  };

  /* -- wizard state ---------------------------------------------- */
  const [step, setStep] = useState(0);
  const [label, setLabel] = useState("");
  const [host, setHost] = useState("");
  const [port, setPort] = useState(String(RR_PORT));
  const [username, setUsername] = useState("");
  /** Operator (default) or station role for the server being added. */
  const [role, setRole] = useState<ClientRole>("operator");
  /** Normal-mode only: whether the user is using their stored default name. */
  const [usingDefaultName, setUsingDefaultName] = useState(true);

  const resetWizard = () => {
    setStep(0);
    setLabel("");
    setHost("");
    setPort(String(RR_PORT));
    setUsername(defaultUsername);
    setRole(defaultRole);
    setUsingDefaultName(true);
    setCertLabel("default");
    setCreatingCert(false);
    setNewCertName("");
  };

  /* -- per-step validation --------------------------------------- */
  const canAdvance = (): boolean => {
    if (step === 0) return host.trim().length > 0;
    if (step === 1) {
      // Happy path: using the stored default name is always valid.
      if (userMode === "normal" && defaultUsername && usingDefaultName) return true;
      return username.trim().length > 0;
    }
    return true; // label is optional
  };

  /* -- actions --------------------------------------------------- */
  const handleNext = (e: FormEvent) => {
    e.preventDefault();
    if (!canAdvance()) return;
    if (step < STEPS.length - 1) {
      setStep((s) => s + 1);
    }
  };

  const handleBack = () => {
    if (step > 0) setStep((s) => s - 1);
  };

  const handleConnectAndSave = async () => {
    if (!host || !username) return;
    const p = Number.parseInt(port) || RR_PORT;
    const resolvedCert = certLabel || null;
    const entry = await addServer({
      label: label || host,
      host,
      port: p,
      username,
      cert_label: resolvedCert,
      role,
    });
    setSavedServers((prev) => [entry, ...prev]);
    notifyServersChanged();
    await connect(host, p, username, resolvedCert);
  };

  const handleQuickConnectForm = async () => {
    if (!host || !username) return;
    const p = Number.parseInt(port) || RR_PORT;
    await connect(host, p, username, certLabel || null);
  };

  /**
   * Connect to a built-in Remote Radio server. The server is not persisted as
   * a saved entry (the list is part of the product), but we must still supply
   * the shared password and a username. Operators pick their callsign as the
   * default username; if none is set yet we fall back to the host so the
   * connect dialog can prompt.
   */
  const handleBuiltinConnect = useCallback(
    async (server: BuiltinServer, chosenRole: ClientRole) => {
      // Apply the exact Operator/Station choice to THIS connection directly,
      // keyed by host:port, so it is used verbatim by the post-connect role
      // effects (PTT direction, role announcement, station texture, initial
      // voice state) instead of being re-derived and possibly falling back to
      // operator.
      setExplicitRoleForServer(server.host, server.port, chosenRole);

      // Also remember the choice per-host in preferences so a later reconnect
      // (which may not go through this handler) still resolves correctly.
      try {
        const prefs = await getPreferences();
        await updatePreferences({
          builtinRoles: { ...(prefs.builtinRoles ?? {}), [server.host]: chosenRole },
        });
      } catch (e) {
        console.warn("[connect] persist built-in role failed:", e);
      }
      const name = (defaultUsername || "").trim();
      // Each entry carries its own voice port (64738 legacy / 54736 fancy).
      await connect(
        server.host,
        server.port,
        name || server.abbr,
        null,
        RR_PASSWORD,
      );
    },
    [connect, defaultUsername],
  );

  const handleQuickConnect = async (server: SavedServer) => {
    setConnectingServerId(server.id);
    let storedPw = await getServerPassword(server.id);
    if (!storedPw) {
      const sibling = savedServers.find(
        (s) =>
          s.id !== server.id &&
          s.host === server.host &&
          s.port === server.port &&
          s.username === server.username,
      );
      if (sibling) storedPw = await getServerPassword(sibling.id);
    }
    await connect(server.host, server.port, server.username, server.cert_label, storedPw);
  };

  const handleCancelConnect = useCallback(async () => {
    await disconnect();
    setConnectingServerId(null);
  }, [disconnect]);

  const handleDelete = async (id: string) => {
    await removeServer(id);
    notifyServersChanged();
    setSavedServers((prev) => {
      const next = prev.filter((s) => s.id !== id);
      if (next.length === 0) setView("servers");
      return next;
    });
  };

  /* -- server editing ------------------------------------------ */
  const [editingServer, setEditingServer] = useState<SavedServer | null>(null);

  const handleEditServer = (server: SavedServer) => setEditingServer(server);

  const handleSaveEdit = async (
    id: string,
    patch: Partial<Omit<SavedServer, "id">>,
  ) => {
    await updateServer(id, patch);
    setSavedServers((prev) =>
      prev.map((s) => (s.id === id ? { ...s, ...patch } : s)),
    );
    notifyServersChanged();
    setEditingServer(null);
  };

  const handleToggleFavorite = async (id: string) => {
    const server = savedServers.find((s) => s.id === id);
    if (!server) return;
    const next = !server.favorite;
    await updateServer(id, { favorite: next });
    setSavedServers((prev) =>
      prev.map((s) => (s.id === id ? { ...s, favorite: next } : s)),
    );
  };

  /* -- password dialog helpers --------------------------------- */

  /** Find the saved-server id matching the current pendingConnect, if any. */
  const matchingServerId = pendingConnect
    ? (savedServers.find(
        (s) =>
          s.host === pendingConnect.host &&
          s.port === pendingConnect.port &&
          s.username === pendingConnect.username,
      )?.id ?? null)
    : null;

  const handlePasswordSubmit = useCallback(
    async (password: string, save: boolean) => {
      const targetId = connectingServerId ?? matchingServerId;
      if (save && targetId) {
        await setServerPassword(targetId, password);
      }
      retryWithPassword(password);
    },
    [connectingServerId, matchingServerId, retryWithPassword],
  );

  const handleChangeUsername = useCallback(
    async (newUsername: string) => {
      if (!pendingConnect) return;
      const targetId = connectingServerId ?? matchingServerId;
      // Persist the new username on the saved server entry so future
      // quick-connects use it.
      if (targetId) {
        await updateServer(targetId, { username: newUsername });
        setSavedServers((prev) =>
          prev.map((s) => (s.id === targetId ? { ...s, username: newUsername } : s)),
        );
      }
      // Dismiss the password prompt before issuing a fresh connect so the
      // store isn't blocked by a stale passwordRequired flag.
      dismissPasswordPrompt();
      await connect(
        pendingConnect.host,
        pendingConnect.port,
        newUsername,
        pendingConnect.certLabel ?? null,
      );
    },
    [pendingConnect, connectingServerId, matchingServerId, dismissPasswordPrompt, connect],
  );

  const handleShowWizard = () => {
    resetWizard();
    setView("wizard");
  };

  const handleBackToServers = () => {
    setView(savedServers.length > 0 ? "servers" : "wizard");
    resetWizard();
  };

  /* -- render helpers -------------------------------------------- */
  const isLastStep = step === STEPS.length - 1;
  const currentStep = STEPS[step];

  if (view === "loading") return null;

  const cardClass = styles.card;

  return (
    <div className={styles.page}>
      <div className={cardClass}>
        {/* Logo - always visible */}
        <div className={styles.logo}>
          <BrandLogo size={64} className={styles.logoIcon} />
          <h1 className={styles.title}>{t("common:brand")}</h1>
          <p className={styles.subtitle}>
            {view === "servers" ? t("chooseServer") : currentStep.subtitle}
          </p>
        </div>

        {/* Error banner */}
        {error && (
          <div className={styles.error}>
            <span className={styles.errorIcon}>!</span>
            {error}
          </div>
        )}

        {/* -------- Server list view (happy path) -------- */}
        {view === "servers" && (
          <>
            <BuiltinServerList
              onConnect={handleBuiltinConnect}
              defaultRole={defaultRole}
              disabled={isConnecting}
            />
            {savedServers.length > 0 && (
              <ServerList
                servers={savedServers}
                pings={pings}
                onConnect={handleQuickConnect}
                onDelete={handleDelete}
                onToggleFavorite={handleToggleFavorite}
                onEdit={handleEditServer}
                onAddNew={handleShowWizard}
                onCancelConnect={handleCancelConnect}
                disabled={isConnecting}
                connectingId={connectingServerId}
                connectingMessage={bootstrapStage}
              />
            )}
            {editingServer && (
              <ServerEditSheet
                server={editingServer}
                onSave={handleSaveEdit}
                onClose={() => setEditingServer(null)}
              />
            )}
          </>
        )}

        {/* -------- Multi-step wizard -------------------- */}
        {view === "wizard" && (
          <>
            {/* Back navigation */}
            {(savedServers.length > 0 || step > 0) && (
              <button
                className={styles.backLink}
                onClick={step > 0 ? handleBack : handleBackToServers}
                disabled={isConnecting}
                type="button"
              >
                ? {step > 0 ? t("back") : t("backToSavedServers")}
              </button>
            )}

            {/* Step indicator */}
            <div className={styles.stepIndicator}>
              {STEPS.map((_, i) => (
                <div
                  key={`step-${STEPS[i].title}`}
                  className={`${styles.stepDot} ${i <= step ? styles.stepDotActive : ""} ${i === step ? styles.stepDotCurrent : ""}`}
                />
              ))}
              <span className={styles.stepLabel}>
                {t("stepCount", { current: step + 1, total: STEPS.length })}
              </span>
            </div>

            <form
              onSubmit={isLastStep ? (e) => { e.preventDefault(); handleConnectAndSave(); } : handleNext}
              className={styles.form}
            >
              {/* -- Step 0: Server address ---------------- */}
              {step === 0 && (
                <>
                  <div className={styles.field}>
                    <label className={styles.label}>I am connecting as</label>
                    <div className={styles.roleToggle} role="radiogroup" aria-label="Connection role">
                      <button
                        type="button"
                        role="radio"
                        aria-checked={role === "operator"}
                        className={`${styles.roleOption} ${role === "operator" ? styles.roleOptionActive : ""}`}
                        onClick={() => setRole("operator")}
                        disabled={isConnecting}
                      >
                        <span className={styles.roleOptionTitle}>Operator</span>
                        <span className={styles.roleOptionHint}>I operate a station remotely</span>
                      </button>
                      <button
                        type="button"
                        role="radio"
                        aria-checked={role === "station"}
                        className={`${styles.roleOption} ${role === "station" ? styles.roleOptionActive : ""}`}
                        onClick={() => setRole("station")}
                        disabled={isConnecting}
                      >
                        <span className={styles.roleOptionTitle}>Station</span>
                        <span className={styles.roleOptionHint}>This PC is at the radio</span>
                      </button>
                    </div>
                  </div>
                  <div className={styles.field}>
                    <label className={styles.label}>{t("fields.host")}</label>
                    <input
                      className={styles.input}
                      data-testid={TID.connectHostInput}
                      type="text"
                      placeholder={t("fields.hostPlaceholder")}
                      value={host}
                      onChange={(e) => setHost(e.target.value)}
                      disabled={isConnecting}
                      autoFocus
                    />
                  </div>
                  {userMode !== "normal" && (
                    <>
                      <div className={styles.field}>
                        <label className={styles.label}>{t("fields.port")}</label>
                        <input
                          className={styles.input}
                          data-testid={TID.connectPortInput}
                          type="text"
                          placeholder={t("fields.portPlaceholder")}
                          value={port}
                          onChange={(e) => setPort(e.target.value)}
                          disabled={isConnecting}
                        />
                      </div>
                      <div className={styles.field}>
                        <label className={styles.label}>{t("fields.certificate")}</label>
                        <select
                          className={styles.input}
                          value={creatingCert ? "__new__" : certLabel}
                          onChange={(e) => {
                            if (e.target.value === "__new__") {
                              setCreatingCert(true);
                            } else {
                              setCreatingCert(false);
                              setCertLabel(e.target.value);
                            }
                          }}
                          disabled={isConnecting}
                        >
                          <option value="">{t("fields.certNone")}</option>
                          {availableCerts.map((c) => (
                            <option key={c} value={c}>
                              {c === "default" ? t("fields.certAutoGenerated", { name: c }) : c}
                            </option>
                          ))}
                          <option value="__new__">{t("fields.certCreateNew")}</option>
                        </select>
                        {creatingCert && (
                          <div className={styles.newCertRow}>
                            <input
                              className={styles.input}
                              type="text"
                              placeholder={t("fields.identityName")}
                              value={newCertName}
                              onChange={(e) => setNewCertName(e.target.value)}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") {
                                  e.preventDefault();
                                  handleCreateCert();
                                }
                              }}
                              autoFocus
                            />
                            <button
                              type="button"
                              className={styles.buttonGhost}
                              onClick={handleCreateCert}
                              disabled={!newCertName.trim()}
                            >
                              {t("actions.create")}
                            </button>
                            <button
                              type="button"
                              className={styles.buttonGhost}
                              onClick={() => {
                                setCreatingCert(false);
                                setNewCertName("");
                              }}
                            >
                              {t("common:actions.cancel")}
                            </button>
                          </div>
                        )}
                      </div>
                    </>
                  )}
                </>
              )}

              {/* -- Step 1: Username ---------------------- */}
              {step === 1 && (
                <>
                  {/* Normal mode + stored default: happy path confirmation */}
                  {userMode === "normal" && defaultUsername && usingDefaultName ? (
                    <div className={styles.usernameSummary}>
                      <div className={styles.usernameConfirm}>
                        <span className={styles.usernameCheckmark} aria-hidden="true">
                          <CheckIcon width={16} height={16} />
                        </span>
                        <span className={styles.usernameValue}>{defaultUsername}</span>
                      </div>
                      <button
                        type="button"
                        className={styles.usernameOtherLink}
                        onClick={() => {
                          setUsingDefaultName(false);
                          setUsername(defaultUsername);
                        }}
                        disabled={isConnecting}
                      >
                        {t("actions.useDifferentName")}
                      </button>
                    </div>
                  ) : (
                    <div className={styles.field}>
                      <label className={styles.label}>
                        {role === "station" ? "Station Name" : "Operator Callsign"}
                      </label>
                      <input
                        className={styles.input}
                        data-testid={TID.connectUsernameInput}
                        type="text"
                        placeholder={role === "station" ? "Elecraft-K3, etc" : "W1VE, etc"}
                        value={username}
                        onChange={(e) => setUsername(e.target.value)}
                        disabled={isConnecting}
                        autoFocus
                      />
                      {userMode === "normal" && defaultUsername && !usingDefaultName && (
                        <button
                          type="button"
                          className={styles.usernameOtherLink}
                          onClick={() => {
                            setUsingDefaultName(true);
                            setUsername(defaultUsername);
                          }}
                          disabled={isConnecting}
                        >
                          {t("actions.useDefaultName", { name: defaultUsername })}
                        </button>
                      )}
                    </div>
                  )}
                </>
              )}

              {/* -- Step 2: Label (expert only) ---------- */}
              {step === 2 && userMode !== "normal" && (
                <div className={styles.field}>
                  <label className={styles.label}>{t("fields.label")}</label>
                  <input
                    className={styles.input}
                    type="text"
                    placeholder={host || t("fields.labelPlaceholderFallback")}
                    value={label}
                    onChange={(e) => setLabel(e.target.value)}
                    disabled={isConnecting}
                    autoFocus
                  />
                </div>
              )}

              {/* Hint card */}
              <div className={styles.hint}>
                <span className={styles.hintIcon} aria-hidden="true">
                  <InfoFilledIcon width={16} height={16} />
                </span>
                <span>
                  {step === 1 && role === "station"
                    ? t("wizard.stationNameHint")
                    : currentStep.hint}
                </span>
              </div>

              {/* Action buttons */}
              {isLastStep ? (
                <div className={styles.buttonRow}>
                  <button
                    className={styles.buttonGhost}
                    data-testid={TID.quickConnect}
                    type="button"
                    onClick={handleQuickConnectForm}
                    disabled={isConnecting || !host || !username}
                  >
                    {t("actions.quickConnect")}
                  </button>
                  <button
                    className={styles.button}
                    data-testid={TID.connectAndSave}
                    type="submit"
                    disabled={isConnecting || !host || !username}
                  >
                    {isConnecting ? (
                      <>
                        <span className={styles.spinner} />
                        {t("actions.connecting")}
                      </>
                    ) : (
                      t("actions.connectAndSave")
                    )}
                  </button>
                </div>
              ) : (
                <button
                  className={styles.button}
                  data-testid={TID.wizardContinue}
                  type="submit"
                  disabled={!canAdvance()}
                >
                  {t("actions.continue")}
                </button>
              )}
            </form>
          </>
        )}
      </div>

      <PasswordDialog
        open={passwordRequired}
        onSubmit={handlePasswordSubmit}
        onCancel={dismissPasswordPrompt}
        serverHost={pendingConnect?.host}
        username={pendingConnect?.username}
        error={error}
        showSaveOption={matchingServerId !== null}
        onChangeUsername={handleChangeUsername}
      />
    </div>
  );
}
