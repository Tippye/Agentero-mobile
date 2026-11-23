import {
	ExternalLink,
	Globe,
	LoaderCircle,
	RefreshCw,
	TriangleAlert,
} from "lucide-react";
import QRCode from "qrcode";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { SiGithub, SiGoogle, SiZhihu } from "react-icons/si";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type {
	RemoteAgentAccount,
	RemoteAgentCatalog,
	RemoteAgentOAuthProvider,
	RemoteCredentialState,
} from "@/lib/core/bindings";
import { copyTextToClipboard } from "@/lib/core/clipboard";
import { errorText } from "@/lib/core/error";
import { notifyError } from "@/lib/core/notify";
import { openExternalUrl } from "@/lib/core/open-external";
import { isMobileApp, isTauri } from "@/lib/core/tauri";
import { cn } from "@/lib/core/utils";
import { canRetrySavedSignIn, canSignIn } from "@/lib/remote-agent/credentials";
import { remoteAgent } from "@/lib/remote-agent/service";

/** Hosted default: https://github.com/liguobao/ds-harness-remote quick start. */
export const DEFAULT_DSH_REMOTE_SERVER = "https://dsh.r2049.cn";
const POLL_INTERVAL_MS = 2000;
/** Consecutive transient poll failures before the attempt reports an error. */
const MAX_POLL_FAILURES = 3;

/** Same states as the mobile page, keyed into the settings namespace. */
function settingsCredentialMessageKey(
	state: RemoteCredentialState | undefined,
) {
	switch (state) {
		case "notChecked":
			return "agent.dshRemote.credentials.checking" as const;
		case "locked":
			return "agent.dshRemote.credentials.locked" as const;
		case "unavailable":
			return "agent.dshRemote.credentials.unavailable" as const;
		case "invalid":
			return "agent.dshRemote.credentials.invalid" as const;
		case "reauthenticationRequired":
			return "agent.dshRemote.credentials.reauthenticationRequired" as const;
		case "connectionFailed":
			return "agent.dshRemote.credentials.connectionFailed" as const;
		default:
			return null;
	}
}

type SignInPhase =
	| { kind: "idle" }
	| { kind: "starting"; providerId: string }
	| {
			kind: "waiting";
			providerId: string;
			qrId: string;
			scanUrl: string;
			expiresAt: number;
	  }
	| { kind: "completing"; providerId: string }
	| { kind: "expired"; providerId: string };

function providerIcon(id: string, className: string) {
	switch (id) {
		case "github":
			return <SiGithub className={className} aria-hidden />;
		case "google":
			return <SiGoogle className={className} aria-hidden />;
		case "zhihu":
			return (
				<SiZhihu className={cn(className, "text-[#0084FF]")} aria-hidden />
			);
		default:
			return <Globe className={className} aria-hidden />;
	}
}

/** Preferred default order, matching the upstream hosted server. */
function preferredProvider(
	providers: RemoteAgentOAuthProvider[],
): RemoteAgentOAuthProvider | null {
	const order = isMobileApp()
		? ["github", "google", "zhihu"]
		: ["zhihu", "github", "google"];
	for (const id of order) {
		const found = providers.find((provider) => provider.id === id);
		if (found) return found;
	}
	return providers[0] ?? null;
}

export type DshRemoteSignInProps = {
	/** Compact touch-first layout for the mobile shell. */
	mobile?: boolean;
	/** Called once when the native side has activated the signed-in account. */
	onSignedIn: (account: RemoteAgentAccount) => void | Promise<void>;
	/** Optional catalog observer so owners can mirror credential state. */
	onCatalog?: (catalog: RemoteAgentCatalog) => void;
};

/**
 * dsh-remote third-party sign-in (GitHub / Google / Zhihu). The server offers
 * browser sign-in sessions only — there is no account/password form. Desktop
 * shows the authorization QR plus an "open in browser" action; mobile opens
 * the system browser and polls for completion, like dsh-Go.
 */
export function DshRemoteSignIn({
	mobile = false,
	onSignedIn,
	onCatalog,
}: DshRemoteSignInProps) {
	const { t } = useTranslation(["settings", "common"]);
	const serverInputId = useId();
	const [catalog, setCatalog] = useState<RemoteAgentCatalog | null>(null);
	const [server, setServer] = useState(DEFAULT_DSH_REMOTE_SERVER);
	const [providers, setProviders] = useState<RemoteAgentOAuthProvider[] | null>(
		null,
	);
	const [providersFailed, setProvidersFailed] = useState(false);
	const [phase, setPhase] = useState<SignInPhase>({ kind: "idle" });
	const [qrImage, setQrImage] = useState<string | null>(null);
	const [reload, setReload] = useState(0);
	const runRef = useRef(0);
	const mounted = useRef(true);
	const waitingQrRef = useRef<string | null>(null);
	const signedInRef = useRef(onSignedIn);
	signedInRef.current = onSignedIn;
	const catalogRef = useRef(onCatalog);
	catalogRef.current = onCatalog;

	const adoptCatalog = useCallback((value: RemoteAgentCatalog) => {
		setCatalog(value);
		catalogRef.current?.(value);
	}, []);

	useEffect(() => {
		mounted.current = true;
		return () => {
			mounted.current = false;
			runRef.current += 1;
			// Abandon a pending browser session so the native attempt map does
			// not keep it until expiry after the pane closes.
			if (waitingQrRef.current) {
				void remoteAgent.oauthCancel(waitingQrRef.current).catch(() => {});
				waitingQrRef.current = null;
			}
		};
	}, []);

	// Load the native credential state once; a failed read keeps the last view.
	useEffect(() => {
		if (!isTauri()) return;
		let cancelled = false;
		void remoteAgent
			.catalog()
			.then((value) => {
				if (!cancelled) adoptCatalog(value);
			})
			.catch((error) => {
				if (!cancelled) notifyError(errorText(error));
			});
		return () => {
			cancelled = true;
		};
	}, [adoptCatalog]);

	const serverTrimmed = server.trim();
	const signInAllowed = canSignIn(catalog);
	const busy =
		phase.kind === "starting" ||
		phase.kind === "waiting" ||
		phase.kind === "completing";

	// Discover the server's sign-in providers whenever the origin changes.
	// biome-ignore lint/correctness/useExhaustiveDependencies: reload re-runs discovery after an explicit retry
	useEffect(() => {
		if (!isTauri() || !signInAllowed || !serverTrimmed) return;
		let cancelled = false;
		setProviders(null);
		setProvidersFailed(false);
		void remoteAgent
			.oauthProviders(serverTrimmed)
			.then((list) => {
				if (!cancelled) setProviders(list);
			})
			.catch(() => {
				if (!cancelled) {
					setProviders([]);
					setProvidersFailed(true);
				}
			});
		return () => {
			cancelled = true;
		};
	}, [serverTrimmed, signInAllowed, reload]);

	// Changing the server abandons any in-flight attempt.
	const changeServer = useCallback((value: string) => {
		if (waitingQrRef.current) {
			void remoteAgent.oauthCancel(waitingQrRef.current).catch(() => {});
			waitingQrRef.current = null;
		}
		runRef.current += 1;
		setPhase({ kind: "idle" });
		setQrImage(null);
		setServer(value);
	}, []);

	const cancelAttempt = useCallback(() => {
		runRef.current += 1;
		if (waitingQrRef.current) {
			void remoteAgent.oauthCancel(waitingQrRef.current).catch(() => {});
			waitingQrRef.current = null;
		}
		setPhase({ kind: "idle" });
		setQrImage(null);
	}, []);

	const startSignIn = useCallback(
		async (provider: RemoteAgentOAuthProvider) => {
			if (!signInAllowed || busy) return;
			const run = ++runRef.current;
			setPhase({ kind: "starting", providerId: provider.id });
			setQrImage(null);
			try {
				const session = await remoteAgent.oauthStart(
					serverTrimmed,
					provider.id,
				);
				if (!mounted.current || run !== runRef.current) {
					void remoteAgent.oauthCancel(session.qrId).catch(() => {});
					return;
				}
				setPhase({
					kind: "waiting",
					providerId: provider.id,
					qrId: session.qrId,
					scanUrl: session.scanUrl,
					expiresAt: Date.now() + session.expiresIn * 1000,
				});
				waitingQrRef.current = session.qrId;
				if (mobile) {
					openExternalUrl(session.scanUrl);
				} else {
					void QRCode.toDataURL(session.scanUrl, {
						errorCorrectionLevel: "M",
						margin: 1,
						width: 240,
					})
						.then((image) => {
							if (mounted.current && run === runRef.current) setQrImage(image);
						})
						.catch(() => {});
				}
			} catch (error) {
				if (mounted.current && run === runRef.current) {
					setPhase({ kind: "idle" });
					notifyError(errorText(error));
				}
			}
		},
		[signInAllowed, busy, serverTrimmed, mobile],
	);

	// Poll the waiting session; completion activates the account inside Rust.
	const waiting = phase.kind === "waiting" ? phase : null;
	useEffect(() => {
		if (!waiting) return;
		let cancelled = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		let failures = 0;
		const run = runRef.current;
		const finish = (next: SignInPhase) => {
			if (!cancelled && run === runRef.current) setPhase(next);
		};
		const poll = async () => {
			if (cancelled || run !== runRef.current) return;
			if (Date.now() >= waiting.expiresAt) {
				void remoteAgent.oauthCancel(waiting.qrId).catch(() => {});
				waitingQrRef.current = null;
				finish({ kind: "expired", providerId: waiting.providerId });
				return;
			}
			try {
				const status = await remoteAgent.oauthPoll(waiting.qrId);
				if (cancelled || run !== runRef.current) return;
				failures = 0;
				if (status.status === "pending") {
					timer = setTimeout(() => void poll(), POLL_INTERVAL_MS);
					return;
				}
				if (status.status === "expired") {
					waitingQrRef.current = null;
					finish({ kind: "expired", providerId: waiting.providerId });
					return;
				}
				waitingQrRef.current = null;
				setPhase({ kind: "completing", providerId: waiting.providerId });
				setQrImage(null);
				try {
					const value = await remoteAgent.catalog();
					if (!cancelled && run === runRef.current) adoptCatalog(value);
				} catch {
					/* The account view still arrives through onSignedIn. */
				}
				if (!cancelled && run === runRef.current) {
					try {
						await signedInRef.current(status.account);
					} catch (error) {
						// Sign-in itself succeeded; only the owner's follow-up failed.
						notifyError(errorText(error));
					}
				}
			} catch (error) {
				if (cancelled || run !== runRef.current) return;
				failures += 1;
				if (failures >= MAX_POLL_FAILURES) {
					void remoteAgent.oauthCancel(waiting.qrId).catch(() => {});
					waitingQrRef.current = null;
					finish({ kind: "idle" });
					notifyError(errorText(error));
					return;
				}
				timer = setTimeout(() => void poll(), POLL_INTERVAL_MS);
			}
		};
		timer = setTimeout(() => void poll(), POLL_INTERVAL_MS);
		return () => {
			cancelled = true;
			clearTimeout(timer);
		};
	}, [waiting, adoptCatalog]);

	const retrySaved = useCallback(async () => {
		try {
			const value = await remoteAgent.catalog();
			adoptCatalog(value);
			if (value.account) await signedInRef.current(value.account);
		} catch (error) {
			notifyError(errorText(error));
		}
	}, [adoptCatalog]);

	const clearSaved = useCallback(async () => {
		try {
			await remoteAgent.logout();
			adoptCatalog(await remoteAgent.catalog());
			setReload((value) => value + 1);
		} catch (error) {
			notifyError(errorText(error));
		}
	}, [adoptCatalog]);

	const credentialMessage = settingsCredentialMessageKey(
		catalog?.credentialState,
	);
	const buttonSize = mobile ? "min-h-12 w-full justify-start gap-3" : "";
	const preferred = providers ? preferredProvider(providers) : null;

	return (
		<div className={cn("flex flex-col gap-3", mobile ? "p-0" : "px-3.5 py-3")}>
			{credentialMessage && catalog ? (
				<div
					className="space-y-2 rounded-lg border border-amber-500/30 bg-amber-500/5 p-3"
					role="status"
				>
					<p className="text-muted-foreground text-xs leading-relaxed">
						{t(credentialMessage)}
					</p>
					<div className="flex flex-wrap gap-2">
						{canRetrySavedSignIn(catalog.credentialState) ? (
							<Button
								type="button"
								variant="outline"
								size={mobile ? undefined : "sm"}
								className={mobile ? "min-h-11" : undefined}
								onClick={() => void retrySaved()}
							>
								{t("agent.dshRemote.credentials.retry")}
							</Button>
						) : null}
						{catalog.canClearSavedSignIn ? (
							<Button
								type="button"
								variant="outline"
								size={mobile ? undefined : "sm"}
								className={mobile ? "min-h-11" : undefined}
								onClick={() => void clearSaved()}
							>
								{t("agent.dshRemote.credentials.clear")}
							</Button>
						) : null}
					</div>
				</div>
			) : null}
			<label
				htmlFor={serverInputId}
				className={cn(
					"grid gap-1",
					mobile ? "text-sm" : "text-muted-foreground text-xs",
				)}
			>
				{t("agent.dshRemote.server")}
				<Input
					id={serverInputId}
					value={server}
					placeholder={t("agent.dshRemote.serverPlaceholder")}
					onChange={(event) => changeServer(event.target.value)}
					type="url"
					required
					autoCapitalize="none"
					autoCorrect="off"
					spellCheck={false}
					disabled={busy}
					className={mobile ? "min-h-12" : "h-8 font-mono text-xs"}
				/>
			</label>
			{signInAllowed ? (
				providers === null ? (
					<p className="flex items-center gap-2 text-muted-foreground text-xs">
						<LoaderCircle className="size-3.5 animate-spin" aria-hidden />
						{t("agent.dshRemote.providersLoading")}
					</p>
				) : providersFailed || providers.length === 0 ? (
					<div className="flex flex-wrap items-center gap-2 text-muted-foreground text-xs">
						<TriangleAlert className="size-3.5 shrink-0" aria-hidden />
						<span className="min-w-0 flex-1">
							{t(
								providersFailed
									? "agent.dshRemote.providersFailed"
									: "agent.dshRemote.providersEmpty",
							)}
						</span>
						<Button
							type="button"
							variant="outline"
							size={mobile ? undefined : "sm"}
							className={mobile ? "min-h-11" : undefined}
							onClick={() => setReload((value) => value + 1)}
						>
							<RefreshCw className="size-3.5" aria-hidden />
							{t("agent.dshRemote.retryProviders")}
						</Button>
					</div>
				) : phase.kind === "waiting" || phase.kind === "completing" ? (
					<div className="flex flex-col items-center gap-3 py-1" role="status">
						{!mobile && waiting ? (
							qrImage ? (
								<img
									src={qrImage}
									alt={t("agent.dshRemote.qrAlt")}
									className="size-40 rounded-md border"
								/>
							) : (
								<div className="grid size-40 place-items-center rounded-md border">
									<LoaderCircle
										className="size-5 animate-spin text-muted-foreground"
										aria-hidden
									/>
								</div>
							)
						) : null}
						<p className="flex items-center gap-2 text-muted-foreground text-xs">
							<LoaderCircle className="size-3.5 animate-spin" aria-hidden />
							{t(
								phase.kind === "completing"
									? "agent.dshRemote.completing"
									: mobile
										? "agent.dshRemote.waitingMobile"
										: "agent.dshRemote.waiting",
								{
									provider:
										providers.find(
											(provider) => provider.id === phase.providerId,
										)?.displayName ?? phase.providerId,
								},
							)}
						</p>
						{waiting ? (
							<div className="flex flex-wrap items-center justify-center gap-2">
								<Button
									type="button"
									variant="outline"
									size={mobile ? undefined : "sm"}
									className={mobile ? "min-h-11" : undefined}
									onClick={() => openExternalUrl(waiting.scanUrl)}
								>
									<ExternalLink className="size-3.5" aria-hidden />
									{t("agent.dshRemote.openBrowser")}
								</Button>
								{!mobile ? (
									<Button
										type="button"
										variant="ghost"
										size="sm"
										onClick={() =>
											void copyTextToClipboard(waiting.scanUrl, {
												errorMessage: t("agent.dshRemote.copyFailed"),
											})
										}
									>
										{t("agent.dshRemote.copyLink")}
									</Button>
								) : null}
								<Button
									type="button"
									variant="ghost"
									size={mobile ? undefined : "sm"}
									className={mobile ? "min-h-11" : undefined}
									disabled={phase.kind === "completing"}
									onClick={cancelAttempt}
								>
									{t("common:cancel")}
								</Button>
							</div>
						) : null}
					</div>
				) : (
					<div className="flex flex-col gap-2">
						{phase.kind === "expired" ? (
							<p className="text-muted-foreground text-xs" role="status">
								{t("agent.dshRemote.expired")}
							</p>
						) : null}
						{providers.map((provider) => (
							<Button
								key={provider.id}
								type="button"
								variant={provider.id === preferred?.id ? "default" : "outline"}
								size={mobile ? undefined : "sm"}
								className={buttonSize || undefined}
								disabled={busy}
								onClick={() => void startSignIn(provider)}
							>
								{phase.kind === "starting" &&
								phase.providerId === provider.id ? (
									<LoaderCircle className="size-4 animate-spin" aria-hidden />
								) : (
									providerIcon(provider.id, "size-4")
								)}
								{t("agent.dshRemote.signInWith", {
									provider: provider.displayName,
								})}
							</Button>
						))}
						<p className="text-muted-foreground text-xs leading-relaxed">
							{t("agent.dshRemote.browserHint")}
						</p>
					</div>
				)
			) : null}
			{catalog ? (
				<p className="text-muted-foreground text-xs leading-relaxed">
					{t(
						catalog.persistence === "systemStore"
							? "agent.dshRemote.secureSignIn"
							: "agent.dshRemote.launchOnly",
					)}
				</p>
			) : null}
		</div>
	);
}
