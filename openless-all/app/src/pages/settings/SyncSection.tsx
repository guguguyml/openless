import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import {
	getSyncAuthSession,
	getSyncSettings,
	getSyncState,
	setSyncSettings,
	setSyncLastError,
	syncClearCloudData,
	syncClearLocalProfileData,
	syncLogoutDevice,
	syncPull,
	syncPushPending,
	syncRequestEmailCode,
	syncVerifyEmailCode,
} from "../../lib/ipc";
import { emitSaved } from "../../lib/savedEvent";
import type { SyncAuthSession, SyncSettings, SyncState } from "../../lib/types";
import { Btn, Card, Pill } from "../_atoms";
import { SettingRow, inputStyle } from "./shared";

type BusyAction = "code" | "login" | "sync" | "logout" | "clear" | "clearLocal" | "save" | null;

const DEFAULT_SYNC_SERVER_URL = "https://sync.example.com";

export function SyncSection() {
	const { t } = useTranslation();
	const [settings, setSettings] = useState<SyncSettings | null>(null);
	const [state, setState] = useState<SyncState | null>(null);
	const [session, setSession] = useState<SyncAuthSession | null>(null);
	const [email, setEmail] = useState("");
	const [code, setCode] = useState("");
	const [serverUrl, setServerUrl] = useState("");
	const [deviceName, setDeviceName] = useState("");
	const [busy, setBusy] = useState<BusyAction>(null);
	const [codeCooldown, setCodeCooldown] = useState(0);

	const loggedInEmail = session?.accountEmail || settings?.accountEmail || null;
	const isLoggedIn = Boolean(session?.accessToken && loggedInEmail);
	const pendingCount = state?.pendingChanges.length ?? 0;
	const effectiveServerUrl = () => serverUrl.trim() || DEFAULT_SYNC_SERVER_URL;
	const sendCodeDisabled = busy !== null || !email.trim() || codeCooldown > 0;
	const sendCodeLabel =
		busy === "code" ? t("settings.sync.sendingCode") : codeCooldown > 0 ? `${codeCooldown}s` : t("settings.sync.sendCode");

	useEffect(() => {
		if (codeCooldown <= 0) return;
		const timer = window.setTimeout(() => setCodeCooldown(seconds => Math.max(0, seconds - 1)), 1000);
		return () => window.clearTimeout(timer);
	}, [codeCooldown]);

	const lastSyncText = useMemo(() => {
		const value = state?.lastSyncAt ?? state?.lastPullAt ?? state?.lastPushAt ?? null;
		if (!value) return t("settings.sync.never");
		const date = new Date(value);
		if (Number.isNaN(date.getTime())) return value;
		return date.toLocaleString();
	}, [state, t]);

	const refresh = async () => {
		const [nextSettings, nextState, nextSession] = await Promise.all([getSyncSettings(), getSyncState(), getSyncAuthSession()]);
		setSettings(nextSettings);
		setState(nextState);
		setSession(nextSession);
		setEmail(nextSession?.accountEmail ?? nextSettings.accountEmail ?? "");
		setServerUrl(nextSettings.serverUrl || DEFAULT_SYNC_SERVER_URL);
		setDeviceName(nextSettings.deviceName);
	};

	useEffect(() => {
		let cancelled = false;
		(async () => {
			try {
				const [nextSettings, nextState, nextSession] = await Promise.all([
					getSyncSettings(),
					getSyncState(),
					getSyncAuthSession(),
				]);
				if (cancelled) return;
				setSettings(nextSettings);
				setState(nextState);
				setSession(nextSession);
				setEmail(nextSession?.accountEmail ?? nextSettings.accountEmail ?? "");
				setServerUrl(nextSettings.serverUrl || DEFAULT_SYNC_SERVER_URL);
				setDeviceName(nextSettings.deviceName);
			} catch (err) {
				if (!cancelled) emitSaved("failed", formatSyncError(err));
			}
		})();
		return () => {
			cancelled = true;
		};
	}, []);

	const saveSettings = async () => {
		if (!settings) return;
		setBusy("save");
		try {
			const next = await setSyncSettings({
				...settings,
				serverUrl: effectiveServerUrl(),
				deviceName: deviceName.trim() || settings.deviceName,
				accountEmail: loggedInEmail,
			});
			setSettings(next);
			setServerUrl(next.serverUrl || DEFAULT_SYNC_SERVER_URL);
			setDeviceName(next.deviceName);
			emitSaved("saved", t("settings.sync.saved"));
		} catch (err) {
			emitSaved("failed", formatSyncError(err));
		} finally {
			setBusy(null);
		}
	};

	const requestCode = async () => {
		if (!settings) return;
		setBusy("code");
		try {
			const nextSettings = await setSyncSettings({
				...settings,
				serverUrl: effectiveServerUrl(),
				deviceName: deviceName.trim() || settings.deviceName,
				accountEmail: email.trim() || null,
			});
			setSettings(nextSettings);
			const result = await syncRequestEmailCode(email.trim());
			setCodeCooldown(60);
			emitSaved("saved", t("settings.sync.codeSent", { seconds: result.expiresIn }));
		} catch (err) {
			emitSaved("failed", formatSyncError(err));
		} finally {
			setBusy(null);
		}
	};

	const verifyLogin = async () => {
		if (!settings) return;
		setBusy("login");
		try {
			const nextSettings = await setSyncSettings({
				...settings,
				serverUrl: effectiveServerUrl(),
				deviceName: deviceName.trim() || settings.deviceName,
				accountEmail: email.trim() || null,
			});
			setSettings(nextSettings);
			const result = await syncVerifyEmailCode(email.trim(), code.trim());
			const loginMessage = t("settings.sync.loginSuccess", { email: result.accountEmail });
			emitSaved("saved", loginMessage);
			setCode("");
			try {
				const pullResult = await syncPull();
				await syncPushPending();
				emitSaved("saved", `${loginMessage} ${t("settings.sync.syncSuccess", { cursor: pullResult.cursor || "0" })}`);
			} catch (pullError) {
				const formatted = formatSyncError(pullError);
				await setSyncLastError(formatted).catch(error => {
					console.warn("[sync] save login pull error failed", error);
				});
				emitSaved("failed", formatted);
			}
			await refresh();
		} catch (err) {
			emitSaved("failed", formatSyncError(err));
		} finally {
			setBusy(null);
		}
	};

	const manualSync = async () => {
		if (!isLoggedIn) return;
		setBusy("sync");
		try {
			const result = await syncPull();
			const pushed = await syncPushPending();
			await refresh();
			emitSaved("saved", t("settings.sync.syncSuccess", { cursor: pushed.cursor || result.cursor || "0" }));
		} catch (err) {
			emitSaved("failed", formatSyncError(err));
		} finally {
			setBusy(null);
		}
	};

	const logout = async () => {
		if (!isLoggedIn) return;
		setBusy("logout");
		try {
			await syncLogoutDevice();
			await refresh();
			emitSaved("saved", t("settings.sync.logoutSuccess"));
		} catch (err) {
			emitSaved("failed", formatSyncError(err));
		} finally {
			setBusy(null);
		}
	};

	const clearCloud = async () => {
		if (!isLoggedIn) return;
		if (!window.confirm(t("settings.sync.clearConfirmFirst"))) return;
		if (!window.confirm(t("settings.sync.clearConfirmSecond"))) return;
		setBusy("clear");
		try {
			await syncClearCloudData();
			await refresh();
			emitSaved("saved", t("settings.sync.clearSuccess"));
		} catch (err) {
			emitSaved("failed", formatSyncError(err));
		} finally {
			setBusy(null);
		}
	};

	const clearLocal = async () => {
		if (!window.confirm(t("settings.sync.clearLocalConfirm"))) return;
		if (!window.confirm(t("settings.sync.clearLocalConfirmSecond"))) return;
		setBusy("clearLocal");
		try {
			await syncClearLocalProfileData();
			await refresh();
			emitSaved("saved", t("settings.sync.clearLocalSuccess"));
		} catch (err) {
			emitSaved("failed", formatSyncError(err));
		} finally {
			setBusy(null);
		}
	};

	if (!settings || !state) {
		return (
			<Card>
				<div style={{ fontSize: 12, color: "var(--ol-ink-4)" }}>{t("common.loading")}</div>
			</Card>
		);
	}

	return (
		<>
			<Card>
				<div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 12, marginBottom: 4 }}>
					<div>
						<div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>{t("settings.sync.title")}</div>
						<div style={{ fontSize: 11.5, color: "var(--ol-ink-4)", lineHeight: 1.5 }}>{t("settings.sync.desc")}</div>
					</div>
					<Pill tone={isLoggedIn ? "ok" : "outline"}>
						{isLoggedIn ? t("settings.sync.signedIn") : t("settings.sync.signedOut")}
					</Pill>
				</div>

				<SettingRow label={t("settings.sync.deviceNameLabel")} desc={t("settings.sync.deviceNameDesc")}>
					<div style={{ display: "flex", gap: 6, width: "100%", maxWidth: 380 }}>
						<input
							type="text"
							value={deviceName}
							onChange={event => setDeviceName(event.target.value)}
							style={{ ...inputStyle, minWidth: 0 }}
						/>
						<Btn size="sm" onClick={() => void saveSettings()} disabled={busy !== null}>
							{busy === "save" ? t("common.saving") : t("settings.sync.saveSettings")}
						</Btn>
					</div>
				</SettingRow>

				<SettingRow label={t("settings.sync.accountLabel")} desc={t("settings.sync.accountDesc")}>
					{isLoggedIn ? (
						<div style={{ display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
							<span
								style={{
									fontSize: 12.5,
									color: "var(--ol-ink)",
									overflow: "hidden",
									textOverflow: "ellipsis",
									whiteSpace: "nowrap",
								}}
							>
								{loggedInEmail}
							</span>
							<Btn size="sm" onClick={() => void logout()} disabled={busy !== null}>
								{busy === "logout" ? t("settings.sync.loggingOut") : t("settings.sync.logout")}
							</Btn>
						</div>
					) : (
						<div style={{ width: "100%", maxWidth: 380 }}>
							<input
								type="text"
								value={email}
								onChange={event => setEmail(event.target.value)}
								placeholder={t("settings.sync.emailPlaceholder")}
								style={{ ...inputStyle, width: "100%", marginBottom: 6 }}
							/>
							<div style={{ display: "flex", gap: 6, width: "100%", minWidth: 0 }}>
								<input
									type="text"
									inputMode="numeric"
									value={code}
									onChange={event => setCode(event.target.value)}
									placeholder={t("settings.sync.codePlaceholder")}
									style={{ ...inputStyle, minWidth: 20, width: "100%" }}
								/>
								<Btn size="sm" onClick={() => void requestCode()} disabled={sendCodeDisabled}>
									{sendCodeLabel}
								</Btn>
								<Btn
									size="sm"
									variant="blue"
									onClick={() => void verifyLogin()}
									disabled={busy !== null || !email.trim() || !code.trim()}
								>
									{busy === "login" ? t("settings.sync.loggingIn") : t("settings.sync.login")}
								</Btn>
							</div>
						</div>
					)}
				</SettingRow>
			</Card>

			<Card>
				<div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>{t("settings.sync.statusTitle")}</div>
				<div style={{ fontSize: 11.5, color: "var(--ol-ink-4)", marginBottom: 6 }}>{t("settings.sync.statusDesc")}</div>
				<SettingRow label={t("settings.sync.lastSyncLabel")} desc={t("settings.sync.lastSyncDesc")}>
					<span style={{ fontSize: 12.5, color: "var(--ol-ink-2)" }}>{lastSyncText}</span>
				</SettingRow>
				<SettingRow label={t("settings.sync.pendingLabel")} desc={t("settings.sync.pendingDesc")}>
					<span style={{ fontSize: 12.5, color: "var(--ol-ink-2)" }}>{t("settings.sync.pendingCount", { count: pendingCount })}</span>
				</SettingRow>
				<SettingRow label={t("settings.sync.manualSyncLabel")} desc={t("settings.sync.manualSyncDesc")}>
					<Btn variant="blue" size="sm" onClick={() => void manualSync()} disabled={!isLoggedIn || busy !== null}>
						{busy === "sync" ? t("settings.sync.syncing") : t("settings.sync.manualSync")}
					</Btn>
				</SettingRow>
				<SettingRow label={t("settings.sync.clearCloudLabel")} desc={t("settings.sync.clearCloudDesc")}>
					<Btn size="sm" onClick={() => void clearCloud()} disabled={!isLoggedIn || busy !== null}>
						{busy === "clear" ? t("settings.sync.clearing") : t("settings.sync.clearCloud")}
					</Btn>
				</SettingRow>
				<SettingRow label={t("settings.sync.clearLocalLabel")} desc={t("settings.sync.clearLocalDesc")}>
					<Btn size="sm" onClick={() => void clearLocal()} disabled={busy !== null}>
						{busy === "clearLocal" ? t("settings.sync.clearing") : t("settings.sync.clearLocal")}
					</Btn>
				</SettingRow>
			</Card>

			<Card>
				<div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>{t("settings.sync.advancedTitle")}</div>
				<div style={{ fontSize: 11.5, color: "var(--ol-ink-4)", marginBottom: 6, lineHeight: 1.5 }}>
					{t("settings.sync.advancedDesc")}
				</div>
				<SettingRow label={t("settings.sync.serverUrlLabel")} desc={t("settings.sync.serverUrlDesc")}>
					<div style={{ display: "flex", gap: 6, width: "100%", maxWidth: 520 }}>
						<input
							type="url"
							value={serverUrl}
							onChange={event => setServerUrl(event.target.value)}
							placeholder={DEFAULT_SYNC_SERVER_URL}
							style={{ ...inputStyle, minWidth: 0, maxWidth: "none" }}
						/>
						<Btn size="sm" onClick={() => void saveSettings()} disabled={busy !== null}>
							{busy === "save" ? t("common.saving") : t("settings.sync.saveSettings")}
						</Btn>
					</div>
				</SettingRow>
				<SettingRow label={t("settings.sync.migrationLabel")} desc={t("settings.sync.migrationDesc")}>
					<div style={{ display: "flex", flexDirection: "column", gap: 6, width: "100%" }}>
						<div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
							<Btn size="sm" disabled>
								{t("settings.sync.migrationExport")}
							</Btn>
							<Btn size="sm" disabled>
								{t("settings.sync.migrationReupload")}
							</Btn>
							<Btn size="sm" disabled>
								{t("settings.sync.migrationImport")}
							</Btn>
						</div>
						<div style={{ fontSize: 11.5, color: "var(--ol-ink-4)", lineHeight: 1.5 }}>
							{t("settings.sync.migrationComingSoon")}
						</div>
					</div>
				</SettingRow>
			</Card>
		</>
	);
}

function formatSyncError(err: unknown): string {
	if (err instanceof Error) return err.message;
	if (err && typeof err === "object") {
		const value = err as { message?: unknown; code?: unknown; status?: unknown };
		const code = typeof value.code === "string" ? value.code : null;
		const message = typeof value.message === "string" ? value.message : null;
		const status = typeof value.status === "number" ? value.status : null;
		if (message && code && status) return `${message} (${code}, HTTP ${status})`;
		if (message && code) return `${message} (${code})`;
		if (message) return message;
	}
	return String(err);
}
