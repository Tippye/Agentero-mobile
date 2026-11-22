import { ChevronRight, CloudUpload, LoaderCircle, Unplug } from "lucide-react";
import type { ComponentProps, ComponentType } from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FaAws } from "react-icons/fa6";
import {
	SiAlibabacloud,
	SiBaidu,
	SiCloudflare,
	SiGithub,
	SiGitlab,
	SiMinio,
	SiNextcloud,
} from "react-icons/si";
import { JianguoyunIcon } from "@/components/icons/jianguoyun-icon";
import {
	CloudUsageLine,
	useCloudUsage,
} from "@/components/settings/cloud-usage";
import {
	HelpLabel,
	PageTitle,
	SettingsGroup,
	SettingsRow,
	SettingsSectionLabel,
} from "@/components/settings/settings-layout";
import { Button } from "@/components/ui/button";
import {
	Collapsible,
	CollapsibleContent,
	CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
	Tooltip,
	TooltipContent,
	TooltipTrigger,
} from "@/components/ui/tooltip";
import { formatLocaleTimestamp } from "@/i18n";
import { formatBytes } from "@/lib/core/background-tasks";
import { events } from "@/lib/core/bindings";
import type { ApiError } from "@/lib/core/ipc";
import {
	errorMessage,
	isLikelyNetworkFailure,
	notifyError,
	notifySuccess,
} from "@/lib/core/notify";
import { openExternalUrl } from "@/lib/core/open-external";
import { displayPath } from "@/lib/core/path";
import { isTauri } from "@/lib/core/tauri";
import { toSafeDisposer } from "@/lib/core/tauri-events";
import { cn } from "@/lib/core/utils";
import {
	emptySyncConfig,
	isSyncProgressPhase,
	SYNC_INTERVAL_CHOICES,
	type SyncBackendConfig,
	type SyncCloudUsage,
	type SyncProgressEvent,
	type SyncScopeSizes,
	type SyncStateEvent,
	type SyncStatus,
	syncConfigure,
	syncDisconnect,
	syncGetStatus,
	syncNow,
	syncResolveConflicts,
	syncScopeSizes,
} from "@/lib/sync/api";
import { vaultDisplayName } from "@/lib/vault/path";
import { isRemoteVaultHandle } from "@/lib/vault/remote/remote-vault";
import { getRecentVaults } from "@/lib/vault/session";
import { SyncConflictDialog } from "./sync-conflict-dialog";

type SyncProviderLink = {
	id: string;
	name: string;
	docsUrl: string;
	icon: ComponentType<{ className?: string }>;
	iconClassName: string;
};

const SYNC_PROVIDER_LINKS: SyncProviderLink[] = [
	{
		id: "aws",
		name: "AWS S3",
		docsUrl:
			"https://docs.aws.amazon.com/AmazonS3/latest/userguide/GetStartedWithS3.html",
		icon: FaAws,
		iconClassName: "text-[#FF9900]",
	},
	{
		id: "cloudflare-r2",
		name: "Cloudflare R2",
		docsUrl: "https://developers.cloudflare.com/r2/get-started/s3/",
		icon: SiCloudflare,
		iconClassName: "text-[#F38020]",
	},
	{
		id: "minio",
		name: "MinIO",
		docsUrl:
			"https://docs.min.io/aistor/administration/console/security-and-access/",
		icon: SiMinio,
		iconClassName: "text-[#C72E49]",
	},
	{
		id: "alibaba-oss",
		name: "Alibaba Cloud OSS",
		docsUrl:
			"https://www.alibabacloud.com/help/en/oss/developer-reference/use-aws-sdks-to-access-oss",
		icon: SiAlibabacloud,
		iconClassName: "text-[#FF6A00]",
	},
	{
		id: "baidu-bos",
		name: "Baidu BOS",
		docsUrl: "https://cloud.baidu.com/doc/BOS/s/ojwvyq973",
		icon: SiBaidu,
		iconClassName: "text-[#2932E1]",
	},
];

const WEBDAV_PROVIDER_LINKS: SyncProviderLink[] = [
	{
		id: "jianguoyun",
		name: "Jianguoyun",
		docsUrl: "https://help.jianguoyun.com/?p=2064",
		icon: JianguoyunIcon,
		iconClassName: "text-[#C7792B]",
	},
	{
		id: "nextcloud",
		name: "Nextcloud",
		docsUrl:
			"https://docs.nextcloud.com/server/latest/user_manual/en/files/access_webdav.html",
		icon: SiNextcloud,
		iconClassName: "text-[#0082C9]",
	},
];

const GIT_PROVIDER_LINKS: SyncProviderLink[] = [
	{
		id: "github",
		name: "GitHub",
		docsUrl:
			"https://docs.github.com/en/repositories/creating-and-managing-repositories/creating-a-new-repository",
		icon: SiGithub,
		iconClassName: "text-foreground",
	},
	{
		id: "gitlab",
		name: "GitLab",
		docsUrl: "https://docs.gitlab.com/ee/user/project/repository/",
		icon: SiGitlab,
		iconClassName: "text-[#FC6D26]",
	},
];

const BACKEND_OPTIONS = [
	{ value: "agentero", labelKey: "sync.backendAgentero" },
	{ value: "s3", labelKey: "sync.backendS3" },
	{ value: "webdav", labelKey: "sync.backendWebdav" },
	{ value: "git", labelKey: "sync.backendGit" },
] as const;

/** Local Vaults this pane can edit: the opener plus the shared MRU list. */
function localVaultChoices(primary: string | null, mobile: boolean): string[] {
	const out: string[] = [];
	const seen = new Set<string>();
	for (const candidate of mobile
		? [primary]
		: [primary, ...getRecentVaults()]) {
		if (!candidate || isRemoteVaultHandle(candidate)) continue;
		const key = candidate.replace(/[\\/]+$/, "");
		if (seen.has(key)) continue;
		seen.add(key);
		out.push(candidate);
	}
	return out;
}

export function SyncPane({
	vaultPath,
	mobile = false,
	onSynced,
}: {
	vaultPath: string | null;
	mobile?: boolean;
	/** Refresh the caller's local catalog after a successful manual sync. */
	onSynced?: () => void | Promise<void>;
}) {
	const [vaults, setVaults] = useState(() =>
		localVaultChoices(vaultPath, mobile),
	);
	const [selectedVault, setSelectedVault] = useState<string | null>(
		() => localVaultChoices(vaultPath, mobile)[0] ?? null,
	);

	useEffect(() => {
		const choices = localVaultChoices(vaultPath, mobile);
		setVaults(choices);
		setSelectedVault((prev) => {
			if (vaultPath && choices.includes(vaultPath)) return vaultPath;
			if (prev && choices.includes(prev)) return prev;
			return choices[0] ?? null;
		});
	}, [vaultPath, mobile]);

	useEffect(() => {
		const onStorage = () => setVaults(localVaultChoices(vaultPath, mobile));
		window.addEventListener("storage", onStorage);
		return () => window.removeEventListener("storage", onStorage);
	}, [vaultPath, mobile]);

	const candidateVault = mobile ? vaultPath : selectedVault;
	const localVault =
		isTauri() && candidateVault && !isRemoteVaultHandle(candidateVault)
			? candidateVault
			: null;
	return (
		<VaultSyncForm
			key={localVault}
			localVault={localVault}
			vaults={vaults}
			setSelectedVault={setSelectedVault}
			mobile={mobile}
			onSynced={onSynced}
		/>
	);
}

function VaultSyncForm({
	localVault,
	vaults,
	setSelectedVault,
	mobile = false,
	onSynced,
}: {
	localVault: string | null;
	vaults: string[];
	setSelectedVault: (vault: string | null) => void;
	mobile?: boolean;
	onSynced?: () => void | Promise<void>;
}) {
	const { t } = useTranslation("settings");
	const { t: tm } = useTranslation("mobile");
	const newConfig = useCallback(() => {
		const config = emptySyncConfig(mobile ? "s3" : "agentero");
		if (mobile) {
			config.scope.source = false;
			// Tablet readers edit the same notes offline: a divergent file is a
			// decision they can act on, not a silent mtime overwrite.
			config.conflictPolicy = "ask";
		}
		return config;
	}, [mobile]);
	const formVersion = useRef(0);
	const requestId = useRef(0);

	const [status, setStatus] = useState<SyncStatus | null>(null);
	const [form, setForm] = useState<SyncBackendConfig>(newConfig);
	const [loading, setLoading] = useState(true);
	const [saving, setSaving] = useState(false);
	const [syncing, setSyncing] = useState(false);
	const [progress, setProgress] = useState<SyncProgressEvent | null>(null);
	const [syncError, setSyncError] = useState<string | null>(null);
	const [advancedOpen, setAdvancedOpen] = useState(false);
	const [scopeSizes, setScopeSizes] = useState<SyncScopeSizes | null>(null);
	const [activeOrgId, setActiveOrgId] = useState("");
	const cloudSubject =
		form.agenteroSubject === "organization" ? "organization" : "user";
	const [cloudUsage, setCloudUsage] = useState<SyncCloudUsage | null>(null);
	/** Divergent-edit files awaiting a per-file choice (ask policy). */
	const [conflictDialogOpen, setConflictDialogOpen] = useState(false);
	/** Auto-open the prompt once per mount; a later dismissal stays dismissed. */
	const conflictPrompted = useRef(false);
	const [resolvingConflicts, setResolvingConflicts] = useState(false);

	const refresh = useCallback(async () => {
		if (!localVault) return;
		const id = ++requestId.current;
		const next = await syncGetStatus(localVault);
		if (id !== requestId.current) return;
		setStatus(next);
		setSyncing(next.running);
		// A pass that paused on a previous visit left the prompt durable; show it
		// once the pane opens instead of waiting for another failed sync.
		if (next.pendingConflicts && !conflictPrompted.current) {
			conflictPrompted.current = true;
			setConflictDialogOpen(true);
		}
		// Preserve an in-progress draft across background status updates; only
		// the first load seeds the form (upstream draft-preservation guard).
		if (formVersion.current !== 0) return;
		formVersion.current++;
		if (
			next.config &&
			(!mobile ||
				next.config.backend === "s3" ||
				next.config.backend === "webdav")
		) {
			setForm(
				next.credentialState === "missing" || next.credentialState === "invalid"
					? { ...next.config, secretKey: "", webdavPassword: "" }
					: next.config,
			);
			// Surface the advanced group when it holds non-default values.
			if (
				next.config.backend === "s3" &&
				(next.config.region !== "us-east-1" ||
					next.config.prefix !== "" ||
					!next.config.forcePathStyle)
			) {
				setAdvancedOpen(true);
			}
		} else {
			setForm(newConfig());
			setAdvancedOpen(false);
		}
	}, [localVault, mobile, newConfig]);

	const loadStatus = useCallback(async () => {
		setLoading(true);
		try {
			await refresh();
			setSyncError(null);
		} catch (error) {
			const message = errorMessage(error);
			setSyncError(message);
			notifyError(message);
		} finally {
			setLoading(false);
		}
	}, [refresh]);

	useEffect(() => {
		setStatus(null);
		setForm(newConfig());
		setSyncError(null);
		setProgress(null);
		setScopeSizes(null);
		setAdvancedOpen(false);
		formVersion.current = 0;
		void loadStatus();
		return () => {
			requestId.current++;
		};
	}, [loadStatus, newConfig]);

	useEffect(() => {
		if (!localVault) return;
		const offs = [
			toSafeDisposer(
				events.syncState.listen((event) => {
					const payload: SyncStateEvent = event.payload;
					if (payload.vaultPath !== localVault) return;
					setSyncing(payload.status === "syncing");
					setSyncError(
						payload.status === "error"
							? (payload.error ?? t("sync.statusError"))
							: null,
					);
					if (payload.status === "error") {
						// The Host retries transport failures on its own; telling
						// the user what to check beats reqwest's error chain.
						const detail = payload.error ?? t("sync.statusError");
						const unreachable = isLikelyNetworkFailure(detail);
						notifyError(unreachable ? t("sync.unreachable") : detail, {
							id: `sync:${localVault}`,
							description: unreachable ? detail : undefined,
						});
					}
					if (payload.status !== "syncing") {
						setProgress(null);
						void refresh().catch(() => undefined);
					}
				}),
			),
			toSafeDisposer(
				events.syncProgress.listen((event) => {
					const { vaultPath, phase, current, total } = event.payload;
					if (vaultPath === localVault && isSyncProgressPhase(phase)) {
						setProgress({ vaultPath, phase, current, total });
					}
				}),
			),
		];
		return () => {
			for (const off of offs) off();
		};
	}, [localVault, refresh, t]);

	useEffect(() => {
		if (!localVault) return;
		let cancelled = false;
		syncScopeSizes(localVault)
			.then((sizes) => {
				if (!cancelled) setScopeSizes(sizes);
			})
			.catch(() => undefined);
		return () => {
			cancelled = true;
		};
	}, [localVault]);

	if (!localVault) {
		return (
			<section>
				<PageTitle title={t("sync.title")} />
				<p className="text-muted-foreground text-sm">
					{t("sync.localVaultOnly")}
				</p>
			</section>
		);
	}

	const patch = (partial: Partial<SyncBackendConfig>) => {
		formVersion.current++;
		setForm((prev) => ({ ...prev, ...partial }));
	};

	const scope = form.scope;
	const patchScope = (key: keyof SyncBackendConfig["scope"], value: boolean) =>
		patch({ scope: { ...scope, [key]: value } });

	const isAgentero = form.backend === "agentero";
	const isWebdav = form.backend === "webdav";
	const isGit = form.backend === "git";
	const credentialBlocked =
		status?.credentialState === "locked" ||
		status?.credentialState === "unavailable";
	const credentialNeedsInput =
		status?.credentialState === "missing" ||
		status?.credentialState === "invalid";
	const secret = (isWebdav ? form.webdavPassword : form.secretKey).trim();
	const secretEntered = Boolean(secret) && secret !== "***";
	const credentialNotice =
		status?.credentialState === "locked"
			? "sync.credentialLocked"
			: status?.credentialState === "unavailable"
				? "sync.credentialUnavailable"
				: status?.credentialState === "missing"
					? "sync.credentialMissing"
					: status?.credentialState === "invalid"
						? "sync.credentialInvalid"
						: null;
	const controlsDisabled =
		loading || !status || saving || syncing || credentialBlocked;
	const backendLocked = status?.configured === true;
	const providerLinks = isAgentero
		? []
		: isGit
			? GIT_PROVIDER_LINKS
			: isWebdav
				? WEBDAV_PROVIDER_LINKS
				: SYNC_PROVIDER_LINKS;
	const cloudBlocked =
		isAgentero &&
		(!cloudUsage?.signedIn ||
			cloudUsage.limit === 0 ||
			(cloudSubject === "organization" && !activeOrgId));

	/** Pending divergent edits, if the last pass paused under the ask policy. */
	const pendingConflicts = status?.pendingConflicts ?? null;

	const resolveConflicts = async (
		resolutions: Record<string, import("@/lib/sync/api").ConflictResolution>,
		defaultResolution: import("@/lib/sync/api").ConflictResolution,
	) => {
		if (!localVault) return;
		setResolvingConflicts(true);
		try {
			const outcome = await syncResolveConflicts(
				localVault,
				resolutions,
				defaultResolution,
			);
			setConflictDialogOpen(false);
			await onSynced?.();
			setSyncError(null);
			notifySuccess(
				t("sync.done", {
					up: outcome.uploaded,
					down: outcome.downloaded,
				}),
			);
		} catch (error) {
			const code =
				error instanceof Error ? (error as ApiError).code : undefined;
			// Still paused on a later conflict set: keep the dialog on the fresh
			// list instead of reporting a failure the user has to clear by hand.
			if (code === "sync_conflict_pending") {
				await refresh().catch(() => undefined);
				return;
			}
			reportSyncError(error);
		} finally {
			setResolvingConflicts(false);
			void refresh().catch(() => undefined);
		}
	};

	const reportSyncError = (error: unknown) => {
		const detail = errorMessage(error);
		const code = error instanceof Error ? (error as ApiError).code : undefined;
		if (code === "git_auth_failed" || code === "git_tls_failed") {
			const message =
				code === "git_auth_failed"
					? t("sync.gitAuthFailed", { detail })
					: t("sync.gitTlsFailed", { detail });
			setSyncError(message);
			notifyError(message);
			return;
		}
		// DNS / connection / timeout failures are retried by the Host and can
		// clear without user action; surface an actionable sentence and keep
		// the raw chain as the secondary line for diagnostics.
		if (code === "sync_unreachable" || isLikelyNetworkFailure(detail)) {
			setSyncError(detail);
			notifyError(t("sync.unreachable"), { description: detail });
			return;
		}
		setSyncError(detail);
		notifyError(detail);
	};

	const save = async () => {
		if (credentialBlocked || (credentialNeedsInput && !secretEntered)) return;
		const version = formVersion.current;
		setSaving(true);
		try {
			const next = await syncConfigure(localVault, form);
			requestId.current++;
			setStatus(next);
			if (next.config && formVersion.current === version) {
				formVersion.current++;
				setForm(next.config);
			}
			setSyncError(null);
			notifySuccess(t("sync.saved"));
		} catch (error) {
			reportSyncError(error);
		} finally {
			setSaving(false);
		}
	};

	const runSync = async () => {
		if (credentialBlocked || credentialNeedsInput) return;
		setSyncing(true);
		try {
			const outcome = await syncNow(localVault);
			await onSynced?.();
			setSyncError(null);
			notifySuccess(
				t("sync.done", {
					up: outcome.uploaded,
					down: outcome.downloaded,
				}),
			);
			if (outcome.conflictCopies.length > 0) {
				notifySuccess(
					t("sync.conflicts", { count: outcome.conflictCopies.length }),
				);
			}
		} catch (error) {
			const code =
				error instanceof Error ? (error as ApiError).code : undefined;
			// Ask policy paused the pass: nothing was applied or published, so
			// surface the per-file prompt instead of an error the user cannot act
			// on. Status is refreshed so the dialog reads the durable list.
			if (code === "sync_conflict_pending") {
				await refresh().catch(() => undefined);
				setConflictDialogOpen(true);
			} else {
				reportSyncError(error);
			}
		} finally {
			setSyncing(false);
			setProgress(null);
			void refresh().catch(() => undefined);
		}
	};

	const disconnect = async () => {
		try {
			await syncDisconnect(localVault);
			requestId.current++;
			formVersion.current++;
			setStatus(null);
			setForm(newConfig());
			setSyncError(null);
			void refresh().catch(() => undefined);
		} catch (error) {
			reportSyncError(error);
		}
	};

	const unsupportedBackend =
		mobile &&
		status?.configured &&
		status.config?.backend !== "s3" &&
		status.config?.backend !== "webdav";
	if (unsupportedBackend)
		return (
			<section>
				<PageTitle title={t("sync.title")} />
				<p className="mb-4 text-muted-foreground text-sm">
					{tm("sync.unsupportedBackend")}
				</p>
				<Button
					type="button"
					variant="outline"
					className="min-h-11"
					onClick={() => void disconnect()}
					disabled={syncing}
				>
					{t("sync.disconnect")}
				</Button>
			</section>
		);

	const field = (
		key: keyof SyncBackendConfig,
		opts?: { type?: string; placeholder?: string },
	) => (
		<Input
			id={`sync-${key}`}
			className={mobile ? "w-full min-w-0 sm:w-64" : "w-64"}
			disabled={controlsDisabled}
			type={opts?.type ?? "text"}
			placeholder={opts?.placeholder}
			value={String(form[key])}
			onChange={(e) => patch({ [key]: e.target.value })}
			autoComplete="off"
			spellCheck={false}
		/>
	);

	const statusText =
		syncing && progress
			? t(`sync.phase.${progress.phase}`, {
					current: progress.current,
					total: progress.total,
				})
			: status?.lastSyncAt
				? t("sync.lastSync", {
						time: formatLocaleTimestamp(status.lastSyncAt),
					})
				: status?.configured
					? t("sync.neverSynced")
					: null;
	const syncStatusLabel =
		syncError || credentialNotice
			? t("sync.statusError")
			: syncing
				? t("sync.statusSyncing")
				: status?.configured
					? t("sync.statusConnected")
					: t("sync.statusDisconnected");
	const syncStatusDotClass =
		syncError || credentialNotice
			? "bg-destructive"
			: syncing
				? "bg-sky-500"
				: status?.configured
					? "bg-emerald-500"
					: "bg-muted-foreground/35";
	const configChanged = JSON.stringify(form) !== JSON.stringify(status?.config);
	const primaryAction = !status ? (
		<Button
			type="button"
			size="sm"
			disabled={loading}
			onClick={() => void loadStatus()}
		>
			{loading ? <LoaderCircle className="animate-spin" /> : null}
			{loading ? tm("sync.loading") : tm("local.retry")}
		</Button>
	) : !status.configured || configChanged || credentialNeedsInput ? (
		<Button
			type="button"
			size="sm"
			onClick={() => void save()}
			disabled={
				loading ||
				saving ||
				syncing ||
				cloudBlocked ||
				credentialBlocked ||
				(credentialNeedsInput && !secretEntered)
			}
		>
			{saving ? (
				<LoaderCircle data-icon="inline-start" className="animate-spin" />
			) : null}
			{isAgentero && !status?.configured
				? t("sync.useAccount")
				: status?.configured
					? t("sync.save")
					: t("sync.connect")}
		</Button>
	) : (
		<Button
			type="button"
			size="sm"
			onClick={() => void runSync()}
			disabled={loading || syncing || credentialBlocked || credentialNeedsInput}
		>
			{syncing ? (
				<LoaderCircle data-icon="inline-start" className="animate-spin" />
			) : (
				<CloudUpload data-icon="inline-start" />
			)}
			{t("sync.syncNow")}
		</Button>
	);

	return (
		<section
			className={
				mobile
					? "[&_input]:min-h-11 [&_button:not([role=switch])]:min-h-11 [&_label]:min-h-11"
					: undefined
			}
		>
			{!mobile && isAgentero ? (
				<DesktopCloudAccount
					subject={cloudSubject}
					orgId={activeOrgId}
					onOrgId={setActiveOrgId}
					onUsage={setCloudUsage}
				/>
			) : null}
			<PageTitle
				title={
					<>
						<span className="inline-flex items-center gap-2">
							{t("sync.title")}
							<span
								role="status"
								aria-label={syncStatusLabel}
								title={syncError ?? syncStatusLabel}
								className={cn("size-2 rounded-full", syncStatusDotClass)}
							/>
						</span>
						{statusText ? (
							<span
								className="ml-2 font-normal text-muted-foreground text-xs"
								role="status"
								aria-live="polite"
							>
								{statusText}
							</span>
						) : null}
					</>
				}
				actions={primaryAction}
			/>
			{syncing && progress ? (
				<progress
					className="mb-4 h-1.5 w-full accent-primary"
					aria-label={statusText ?? t("sync.statusSyncing")}
					max={Math.max(1, progress.total)}
					value={
						progress.total > 0
							? Math.max(0, Math.min(progress.current, progress.total))
							: undefined
					}
				/>
			) : null}

			{credentialNotice ? (
				<div className="mb-5 space-y-3 rounded-xl border bg-muted/30 p-3.5">
					<p className="text-sm">
						{t(credentialNotice, {
							field: t(isWebdav ? "sync.password" : "sync.secretKey"),
						})}
					</p>
					{credentialBlocked ? (
						<Button
							type="button"
							variant="outline"
							disabled={loading}
							onClick={() => void loadStatus()}
						>
							{t("sync.retryCredentials")}
						</Button>
					) : null}
				</div>
			) : null}

			{!mobile ? (
				<div className="mb-3 flex flex-wrap items-center gap-2">
					<span id="sync-vault-label" className="text-muted-foreground text-xs">
						{t("sync.vault")}
					</span>
					<Select
						value={localVault ?? undefined}
						onValueChange={(value) => setSelectedVault(value || null)}
						disabled={vaults.length === 0 || controlsDisabled}
					>
						<SelectTrigger
							id="sync-vault"
							size="sm"
							className="w-44"
							aria-labelledby="sync-vault-label"
						>
							<SelectValue placeholder={t("sync.vaultPlaceholder")}>
								{localVault ? vaultDisplayName(localVault) : undefined}
							</SelectValue>
						</SelectTrigger>
						<SelectContent
							className={mobile ? "[&_[role=option]]:min-h-11" : undefined}
						>
							{vaults.map((path) => (
								<SelectItem key={path} value={path}>
									<span className="flex min-w-0 flex-col items-start">
										<span className="truncate">{vaultDisplayName(path)}</span>
										<span className="truncate text-muted-foreground text-xs">
											{displayPath(path)}
										</span>
									</span>
								</SelectItem>
							))}
						</SelectContent>
					</Select>
				</div>
			) : null}

			{status?.configured && !isGit && !form.conditionalWrites ? (
				<p className="mb-3 text-muted-foreground text-xs">
					{t("sync.noConditionalWrites")}
				</p>
			) : null}

			<div className="mb-3 flex flex-wrap items-center gap-2">
				<span id="sync-backend-label" className="text-muted-foreground text-xs">
					{t("sync.backend")}
				</span>
				<Tooltip>
					<TooltipTrigger asChild>
						<span className="inline-flex">
							<Select
								value={form.backend}
								onValueChange={(value) => {
									if (
										value === "agentero" ||
										value === "s3" ||
										value === "webdav" ||
										value === "git"
									) {
										if (mobile && value !== "s3" && value !== "webdav") return;
										patch({ backend: value });
									}
								}}
								disabled={backendLocked || controlsDisabled}
							>
								<SelectTrigger
									id="sync-backend"
									size="sm"
									className="w-44"
									aria-labelledby="sync-backend-label"
								>
									<SelectValue />
								</SelectTrigger>
								<SelectContent
									className={mobile ? "[&_[role=option]]:min-h-11" : undefined}
								>
									{BACKEND_OPTIONS.filter(
										(option) =>
											!mobile ||
											option.value === "s3" ||
											option.value === "webdav",
									).map((option) => (
										<SelectItem key={option.value} value={option.value}>
											{t(option.labelKey)}
										</SelectItem>
									))}
								</SelectContent>
							</Select>
						</span>
					</TooltipTrigger>
					{backendLocked ? (
						<TooltipContent side="bottom">
							{t("sync.backendLocked")}
						</TooltipContent>
					) : null}
				</Tooltip>
				<div className="ml-auto flex flex-wrap items-center gap-1.5">
					{providerLinks.map((provider) => {
						const Icon = provider.icon;
						return (
							<Tooltip key={provider.id}>
								<TooltipTrigger asChild>
									<Button
										type="button"
										size="icon-lg"
										variant="outline"
										className="size-9 bg-background"
										aria-label={t("sync.providerDocsAria", {
											name: provider.name,
										})}
										onClick={() => openExternalUrl(provider.docsUrl)}
									>
										<Icon
											className={cn("size-5", provider.iconClassName)}
											aria-hidden
										/>
									</Button>
								</TooltipTrigger>
								<TooltipContent side="bottom">{provider.name}</TooltipContent>
							</Tooltip>
						);
					})}
				</div>
			</div>

			{isAgentero ? (
				<SettingsGroup>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.space")}
						htmlFor="sync-space"
					>
						<Select
							value={form.agenteroSubject}
							onValueChange={(value) => {
								if (value !== "user" && value !== "organization") return;
								patch({
									agenteroSubject: value,
									agenteroOrgId: value === "organization" ? activeOrgId : "",
								});
							}}
							disabled={backendLocked || controlsDisabled}
						>
							<SelectTrigger id="sync-space" size="sm" className="w-44">
								<SelectValue />
							</SelectTrigger>
							<SelectContent
								className={mobile ? "[&_[role=option]]:min-h-11" : undefined}
							>
								<SelectItem value="user">{t("sync.spacePersonal")}</SelectItem>
								<SelectItem value="organization" disabled={!activeOrgId}>
									{t("sync.spaceTeam")}
								</SelectItem>
							</SelectContent>
						</Select>
					</SyncSettingsRow>
					<div className="px-3 py-2">
						<CloudUsageLine
							subject={cloudSubject}
							orgId={
								cloudSubject === "organization"
									? activeOrgId || undefined
									: undefined
							}
						/>
						{cloudSubject === "organization" && !activeOrgId ? (
							<p className="text-muted-foreground text-xs">
								{t("sync.noTeam")}
							</p>
						) : null}
					</div>
				</SettingsGroup>
			) : isGit ? (
				<SettingsGroup>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.gitRemote")}
						description={t("sync.gitRemoteHint")}
						htmlFor="sync-gitRemote"
					>
						{field("gitRemote", {
							placeholder: "https://git.example.com/you/papers.git",
						})}
					</SyncSettingsRow>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.gitBranch")}
						htmlFor="sync-gitBranch"
					>
						{field("gitBranch", { placeholder: "main" })}
					</SyncSettingsRow>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.gitAllowInsecureHttp")}
						description={t("sync.gitAllowInsecureHttpHint")}
						htmlFor="sync-gitAllowInsecureHttp"
					>
						<Switch
							id="sync-gitAllowInsecureHttp"
							checked={form.gitAllowInsecureHttp}
							onCheckedChange={(checked) =>
								patch({ gitAllowInsecureHttp: checked })
							}
						/>
					</SyncSettingsRow>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.gitSslCa")}
						description={t("sync.gitSslCaHint")}
						htmlFor="sync-gitSslCaInfo"
					>
						{field("gitSslCaInfo", { placeholder: "/path/to/ca.pem" })}
					</SyncSettingsRow>
				</SettingsGroup>
			) : isWebdav ? (
				<SettingsGroup>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.serverUrl")}
						htmlFor="sync-webdavUrl"
					>
						{field("webdavUrl", {
							placeholder: "https://dav.jianguoyun.com/dav/agentero",
						})}
					</SyncSettingsRow>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.username")}
						htmlFor="sync-webdavUsername"
					>
						{field("webdavUsername")}
					</SyncSettingsRow>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.password")}
						htmlFor="sync-webdavPassword"
					>
						{field("webdavPassword", { type: "password" })}
					</SyncSettingsRow>
				</SettingsGroup>
			) : (
				<SettingsGroup>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.endpoint")}
						htmlFor="sync-endpoint"
					>
						{field("endpoint", { placeholder: "https://…" })}
					</SyncSettingsRow>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.bucket")}
						htmlFor="sync-bucket"
					>
						{field("bucket")}
					</SyncSettingsRow>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.accessKey")}
						htmlFor="sync-accessKey"
					>
						{field("accessKey")}
					</SyncSettingsRow>
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.secretKey")}
						htmlFor="sync-secretKey"
					>
						{field("secretKey", { type: "password" })}
					</SyncSettingsRow>
				</SettingsGroup>
			)}

			<SettingsGroup>
				<SyncSettingsRow
					mobile={mobile}
					label={t("sync.autoSync")}
					htmlFor="sync-autoSync"
				>
					<Switch
						disabled={controlsDisabled}
						className={mobile ? "after:-inset-y-3.5" : undefined}
						id="sync-autoSync"
						checked={form.autoSync}
						onCheckedChange={(checked) => patch({ autoSync: checked })}
					/>
				</SyncSettingsRow>
				{form.autoSync ? (
					<SyncSettingsRow
						mobile={mobile}
						label={t("sync.interval")}
						htmlFor="sync-interval"
					>
						<Select
							disabled={controlsDisabled}
							value={String(form.intervalMinutes)}
							onValueChange={(value) =>
								patch({ intervalMinutes: Number(value) })
							}
						>
							<SelectTrigger id="sync-interval" size="sm" className="w-32">
								<SelectValue />
							</SelectTrigger>
							<SelectContent
								className={mobile ? "[&_[role=option]]:min-h-11" : undefined}
							>
								{SYNC_INTERVAL_CHOICES.map((minutes) => (
									<SelectItem key={minutes} value={String(minutes)}>
										{t("sync.intervalMinutes", { count: minutes })}
									</SelectItem>
								))}
							</SelectContent>
						</Select>
					</SyncSettingsRow>
				) : null}
			</SettingsGroup>

			<SettingsSectionLabel className="mt-4">
				<HelpLabel
					label={t("sync.scope")}
					help={t(isGit ? "sync.scopeHintGit" : "sync.scopeHint")}
				/>
			</SettingsSectionLabel>
			<SettingsGroup>
				<SyncSettingsRow
					mobile={mobile}
					label={t("sync.scopePdf")}
					htmlFor="sync-scope-pdf"
				>
					<span className="flex items-center gap-2">
						{scopeSizes ? (
							<span className="text-muted-foreground text-xs tabular-nums">
								{formatBytes(scopeSizes.pdf)}
							</span>
						) : null}
						<Switch
							disabled={controlsDisabled}
							className={mobile ? "after:-inset-y-3.5" : undefined}
							id="sync-scope-pdf"
							checked={scope.pdf}
							onCheckedChange={(checked) => patchScope("pdf", checked)}
						/>
					</span>
				</SyncSettingsRow>
				<SyncSettingsRow
					mobile={mobile}
					label={t("sync.scopeSource")}
					htmlFor="sync-scope-source"
				>
					<span className="flex items-center gap-2">
						{scopeSizes ? (
							<span className="text-muted-foreground text-xs tabular-nums">
								{formatBytes(scopeSizes.source)}
							</span>
						) : null}
						<Switch
							disabled={controlsDisabled}
							className={mobile ? "after:-inset-y-3.5" : undefined}
							id="sync-scope-source"
							checked={scope.source}
							onCheckedChange={(checked) => patchScope("source", checked)}
						/>
					</span>
				</SyncSettingsRow>
				<SyncSettingsRow
					mobile={mobile}
					label={t("sync.scopeAttachments")}
					htmlFor="sync-scope-attachments"
				>
					<span className="flex items-center gap-2">
						{scopeSizes ? (
							<span className="text-muted-foreground text-xs tabular-nums">
								{formatBytes(scopeSizes.attachments)}
							</span>
						) : null}
						<Switch
							disabled={controlsDisabled}
							className={mobile ? "after:-inset-y-3.5" : undefined}
							id="sync-scope-attachments"
							checked={scope.attachments}
							onCheckedChange={(checked) => patchScope("attachments", checked)}
						/>
					</span>
				</SyncSettingsRow>
				<SyncSettingsRow
					mobile={mobile}
					label={
						<HelpLabel
							label={t("sync.scopeTranslated")}
							help={t("sync.scopeTranslatedHint")}
						/>
					}
					htmlFor="sync-scope-translated"
				>
					<span className="flex items-center gap-2">
						{scopeSizes ? (
							<span className="text-muted-foreground text-xs tabular-nums">
								{formatBytes(scopeSizes.translated ?? 0)}
							</span>
						) : null}
						<Switch
							disabled={controlsDisabled}
							className={mobile ? "after:-inset-y-3.5" : undefined}
							id="sync-scope-translated"
							checked={scope.translated}
							onCheckedChange={(checked) => patchScope("translated", checked)}
						/>
					</span>
				</SyncSettingsRow>
				{mobile ? (
					<SyncSettingsRow
						mobile
						label={t("sync.scopeInk")}
						htmlFor="sync-scope-ink"
					>
						<span className="flex items-center gap-2">
							{scopeSizes ? (
								<span className="text-muted-foreground text-xs tabular-nums">
									{formatBytes(scopeSizes.ink ?? 0)}
								</span>
							) : null}
							<Switch
								disabled={controlsDisabled}
								className="after:-inset-y-3.5"
								id="sync-scope-ink"
								checked={scope.ink}
								onCheckedChange={(checked) => patchScope("ink", checked)}
							/>
						</span>
					</SyncSettingsRow>
				) : null}
			</SettingsGroup>

			{form.backend === "s3" ? (
				<Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen}>
					<CollapsibleTrigger asChild>
						<button
							type="button"
							className="mb-2 flex items-center gap-1 text-muted-foreground text-xs outline-none transition-colors hover:text-foreground"
							aria-label={t("sync.advanced")}
						>
							<ChevronRight
								className={cn(
									"size-3.5 transition-transform",
									advancedOpen && "rotate-90",
								)}
							/>
							{t("sync.advanced")}
						</button>
					</CollapsibleTrigger>
					<CollapsibleContent>
						<SettingsGroup>
							<SyncSettingsRow
								mobile={mobile}
								label={t("sync.region")}
								htmlFor="sync-region"
							>
								{field("region")}
							</SyncSettingsRow>
							<SyncSettingsRow
								mobile={mobile}
								label={
									<HelpLabel
										label={t("sync.prefix")}
										help={t("sync.prefixHint")}
									/>
								}
								htmlFor="sync-prefix"
							>
								{field("prefix")}
							</SyncSettingsRow>
							<SyncSettingsRow
								mobile={mobile}
								label={
									<HelpLabel
										label={t("sync.pathStyle")}
										help={t("sync.pathStyleHint")}
									/>
								}
								htmlFor="sync-pathStyle"
							>
								<Switch
									disabled={controlsDisabled}
									className={mobile ? "after:-inset-y-3.5" : undefined}
									id="sync-pathStyle"
									checked={form.forcePathStyle}
									onCheckedChange={(checked) =>
										patch({ forcePathStyle: checked })
									}
								/>
							</SyncSettingsRow>
						</SettingsGroup>
					</CollapsibleContent>
				</Collapsible>
			) : null}
			<div className="flex items-center gap-2">
				{status?.configured ? (
					<Button
						type="button"
						size="sm"
						variant="ghost"
						onClick={() => void disconnect()}
						disabled={syncing}
					>
						<Unplug data-icon="inline-start" />
						{t("sync.disconnect")}
					</Button>
				) : null}
			</div>
			{/* Ask policy: the paused pass left both sides untouched, so the reader
			    picks per file before anything is applied or published. */}
			<SyncConflictDialog
				pending={conflictDialogOpen ? pendingConflicts : null}
				resolving={resolvingConflicts}
				onResolve={(resolutions, defaultResolution) =>
					void resolveConflicts(resolutions, defaultResolution)
				}
				onDismiss={() => setConflictDialogOpen(false)}
			/>
		</section>
	);
}

/** Desktop account hooks are never mounted by the tablet composition. */
function DesktopCloudAccount({
	subject,
	orgId,
	onOrgId,
	onUsage,
}: {
	subject: "user" | "organization";
	orgId: string;
	onOrgId: (id: string) => void;
	onUsage: (usage: SyncCloudUsage | null) => void;
}) {
	const usage = useCloudUsage(
		subject,
		subject === "organization" ? orgId || undefined : undefined,
	);
	useEffect(() => onUsage(usage), [usage, onUsage]);
	useEffect(() => {
		let active = true;
		void import("@/features/account/client")
			.then(({ authClient }) => authClient.getSession())
			.then((result) => {
				if (active) onOrgId(result.data?.session.activeOrganizationId ?? "");
			})
			.catch((error) => {
				if (active) notifyError(errorMessage(error));
			});
		return () => {
			active = false;
		};
	}, [onOrgId]);
	return null;
}

function SyncSettingsRow({
	mobile,
	className,
	...props
}: ComponentProps<typeof SettingsRow> & { mobile: boolean }) {
	return (
		<SettingsRow
			{...props}
			className={cn(
				mobile &&
					"flex-col items-stretch gap-1 sm:flex-row sm:items-center sm:gap-4 [&>div:last-child]:min-w-0 [&>div:last-child]:w-full [&>div:last-child]:shrink [&>div:last-child]:justify-end sm:[&>div:last-child]:w-auto",
				className,
			)}
		/>
	);
}
