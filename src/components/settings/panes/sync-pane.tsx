import {
	ArrowDownToLine,
	ArrowUpFromLine,
	ChevronRight,
	CloudUpload,
	LoaderCircle,
	Plus,
	Trash2,
	TriangleAlert,
	Unplug,
	X,
} from "lucide-react";
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
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
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
import { readJsonStorage, writeJsonStorage } from "@/lib/core/storage";
import { isTauri } from "@/lib/core/tauri";
import { toSafeDisposer } from "@/lib/core/tauri-events";
import { cn } from "@/lib/core/utils";
import {
	emptySyncConfig,
	isSyncConflictPending,
	isSyncMirrorPhase,
	isSyncProgressPhase,
	SYNC_INTERVAL_CHOICES,
	type SyncBackendConfig,
	type SyncCloudUsage,
	type SyncCredentialState,
	type SyncMirrorPhase,
	type SyncMirrorStatus,
	type SyncOverwriteDirection,
	type SyncOverwritePreview,
	type SyncProgressEvent,
	type SyncScopeSizes,
	type SyncStateEvent,
	type SyncStatus,
	syncConfigure,
	syncDisconnect,
	syncGetStatus,
	syncNow,
	syncOverwrite,
	syncOverwritePreview,
	syncScopeSizes,
} from "@/lib/sync/api";
import { vaultDisplayName } from "@/lib/vault/path";
import { isRemoteVaultHandle } from "@/lib/vault/remote/remote-vault";
import { getRecentVaults } from "@/lib/vault/session";
import { SyncConflictDialog } from "./sync-conflict-dialog";
import { useSyncConflictReview } from "./use-sync-conflict-review";

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

function credentialNoticeKey(state: SyncCredentialState | undefined) {
	switch (state) {
		case "locked":
			return "sync.credentialLocked";
		case "unavailable":
			return "sync.credentialUnavailable";
		case "missing":
			return "sync.credentialMissing";
		case "invalid":
			return "sync.credentialInvalid";
	}
	return null;
}

function credentialBlocked(state: SyncCredentialState | undefined) {
	return state === "locked" || state === "unavailable";
}

function credentialMissing(state: SyncCredentialState | undefined) {
	return state === "missing" || state === "invalid";
}

function secretField(config: SyncBackendConfig) {
	return config.backend === "webdav" ? "webdavPassword" : "secretKey";
}

function secretFieldLabel(config: SyncBackendConfig) {
	return config.backend === "webdav" ? "sync.password" : "sync.secretKey";
}

function isMaskedSecret(value: string | null | undefined) {
	return /^\*+$/.test(value?.trim() ?? "");
}

function hasNewSecret(config: SyncBackendConfig) {
	const secret = config[secretField(config)].trim();
	return secret.length > 0 && !isMaskedSecret(secret);
}

function resetSavedCredentials(config: SyncBackendConfig) {
	const field = secretField(config);
	return {
		...config,
		credentialRef: null,
		[field]: isMaskedSecret(config[field]) ? "" : config[field],
		sessionToken: isMaskedSecret(config.sessionToken)
			? null
			: config.sessionToken,
	};
}

function editableConfig(
	config: SyncBackendConfig,
	state: SyncCredentialState | undefined,
) {
	return credentialMissing(state)
		? { ...config, [secretField(config)]: "", sessionToken: null }
		: config;
}

/** Dismissing the conditional-writes notice is a permanent, device-local choice. */
const DEGRADED_NOTICE_DISMISSED_KEY =
	"agentero.sync.degradedNoticeDismissed.v1";

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
	onReviewConflicts,
}: {
	vaultPath: string | null;
	mobile?: boolean;
	/** Refresh the caller's local catalog after a successful manual sync. */
	onSynced?: () => void | Promise<void>;
	onReviewConflicts?: () => void;
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
			onReviewConflicts={onReviewConflicts}
		/>
	);
}

function VaultSyncForm({
	localVault,
	vaults,
	setSelectedVault,
	mobile = false,
	onSynced,
	onReviewConflicts,
}: {
	localVault: string | null;
	vaults: string[];
	setSelectedVault: (vault: string | null) => void;
	mobile?: boolean;
	onSynced?: () => void | Promise<void>;
	onReviewConflicts?: () => void;
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
	const formDirty = useRef(false);
	const requestId = useRef(0);

	const [status, setStatus] = useState<SyncStatus | null>(null);
	const [form, setForm] = useState<SyncBackendConfig>(newConfig);
	const [loading, setLoading] = useState(true);
	const [saving, setSaving] = useState(false);
	const [syncing, setSyncing] = useState(false);
	const [progress, setProgress] = useState<SyncProgressEvent | null>(null);
	const [syncError, setSyncError] = useState<string | null>(null);
	const [advancedOpen, setAdvancedOpen] = useState(false);
	const [degradedNoticeDismissed, setDegradedNoticeDismissed] = useState(() =>
		readJsonStorage<boolean>(DEGRADED_NOTICE_DISMISSED_KEY, false),
	);
	const [overwriteDialog, setOverwriteDialog] = useState<{
		direction: SyncOverwriteDirection;
		preview: SyncOverwritePreview;
	} | null>(null);
	const [overwriting, setOverwriting] = useState(false);
	const [scopeSizes, setScopeSizes] = useState<SyncScopeSizes | null>(null);
	const [mirrors, setMirrors] = useState<SyncBackendConfig[]>([]);
	const mirrorsDirty = useRef(false);
	// Live per-mirror state from `sync:mirror`, replaced by the persisted
	// `lastSyncAt` / `lastError` on the next status refresh.
	const [mirrorLive, setMirrorLive] = useState<MirrorLive>({});
	const [activeOrgId, setActiveOrgId] = useState("");
	const cloudSubject =
		form.agenteroSubject === "organization" ? "organization" : "user";
	const [cloudUsage, setCloudUsage] = useState<SyncCloudUsage | null>(null);
	const pendingConflicts = status?.pendingConflicts ?? null;
	const describeSyncError = useCallback(
		(detail: string, code?: string | null) => {
			if (code === "sync_no_conditional_writes")
				return t("sync.noConditionalWrites");
			if (code === "sync_mirror_no_conditional_writes")
				return t("sync.mirrorNoConditionalWrites");
			if (code === "sync_credentials_target_changed")
				return t("sync.credentialTargetChanged");
			if (code === "git_auth_failed")
				return t("sync.gitAuthFailed", { detail });
			if (code === "git_tls_failed") return t("sync.gitTlsFailed", { detail });
			if (code === "sync_unreachable" || isLikelyNetworkFailure(detail))
				return t("sync.unreachable");
			return detail;
		},
		[t],
	);

	const refresh = useCallback(async () => {
		if (!localVault) return;
		const id = ++requestId.current;
		const next = await syncGetStatus(localVault);
		if (id !== requestId.current) return;
		setStatus(next);
		setSyncing(next.running);
		if (!mirrorsDirty.current) {
			setMirrors(
				next.mirrors.map(({ config, credentialState }) =>
					editableConfig(config, credentialState),
				),
			);
		}
		// Background status updates can change capabilities or credential state.
		// Keep any in-progress edit until it has been saved.
		if (formDirty.current) return;
		if (
			next.config &&
			(!mobile ||
				next.config.backend === "s3" ||
				next.config.backend === "webdav")
		) {
			setForm(editableConfig(next.config, next.credentialState));
			// Surface the advanced group when it holds non-default values.
			if (
				next.config.backend === "s3" &&
				(next.config.region !== "us-east-1" ||
					next.config.prefix !== "" ||
					!next.config.forcePathStyle ||
					Boolean(next.config.sessionToken))
			) {
				setAdvancedOpen(true);
			}
		} else {
			setForm(newConfig());
			setAdvancedOpen(false);
		}
	}, [localVault, mobile, newConfig]);
	const conflictReview = useSyncConflictReview({
		vaultPath: localVault,
		pending: onReviewConflicts ? null : pendingConflicts,
		refresh,
		onSynced: async () => {
			setSyncError(null);
			await onSynced?.();
		},
		onError: reportSyncError,
	});
	const reviewPending = onReviewConflicts ?? conflictReview.reopen;

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
		formDirty.current = false;
		mirrorsDirty.current = false;
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
					if (payload.status !== "syncing") {
						setProgress(null);
						setMirrorLive({});
						void refresh().catch((error) =>
							notifyError(errorMessage(error), { id: `sync:${localVault}` }),
						);
					}
					// The mobile shell already reports vault-level background errors.
					if (payload.status === "error" && !onReviewConflicts) {
						const detail = payload.error ?? t("sync.statusError");
						const message = describeSyncError(detail, payload.errorCode);
						notifyError(message, {
							id: `sync:${localVault}`,
							...(message === detail ? {} : { description: detail }),
						});
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
			toSafeDisposer(
				events.syncMirror.listen((event) => {
					const payload = event.payload;
					if (payload.vaultPath !== localVault) return;
					const { status } = payload;
					if (!isSyncMirrorPhase(status)) return;
					setMirrorLive((prev) => ({
						...prev,
						[payload.index]: {
							status,
							error: payload.error ?? undefined,
						},
					}));
					if (payload.status !== "started") {
						void refresh().catch(() => undefined);
					}
				}),
			),
		];
		return () => {
			for (const off of offs) off();
		};
	}, [describeSyncError, localVault, onReviewConflicts, refresh, t]);

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
		formDirty.current = true;
		setForm((prev) => {
			const next = { ...prev, ...partial };
			return prev.credentialRef && !sameSyncTarget(prev, next)
				? resetSavedCredentials(next)
				: next;
		});
	};

	const scope = form.scope;
	const patchScope = (key: keyof SyncBackendConfig["scope"], value: boolean) =>
		patch({ scope: { ...scope, [key]: value } });

	const isAgentero = form.backend === "agentero";
	const isWebdav = form.backend === "webdav";
	const isGit = form.backend === "git";
	const primaryCredentialBlocked = credentialBlocked(status?.credentialState);
	const credentialNeedsInput = credentialMissing(status?.credentialState);
	const controlsDisabled =
		loading || !status || saving || syncing || overwriting;
	const primaryFieldsDisabled = controlsDisabled || primaryCredentialBlocked;
	const scopeDisabled = primaryFieldsDisabled || Boolean(pendingConflicts);
	const savedMirrors = status?.mirrors ?? [];
	const mirrorCredentialsIncomplete = mirrors.some((mirror) => {
		if (mobile && mirror.backend === "git") return true;
		if (mirror.backend === "git") return false;
		const saved = savedMirrors.find((entry) =>
			sameSyncTarget(entry.config, mirror),
		);
		if (credentialBlocked(saved?.credentialState)) return true;
		return (
			!hasNewSecret(mirror) &&
			!(
				saved &&
				!credentialMissing(saved.credentialState) &&
				isMaskedSecret(mirror[secretField(mirror)])
			)
		);
	});
	const savedPrimary = status?.config;
	const primaryTargetChanged = Boolean(
		savedPrimary && !sameSyncTarget(savedPrimary, form),
	);
	const requiresReplacement =
		credentialNeedsInput ||
		Boolean(savedPrimary?.credentialRef && primaryTargetChanged);
	const credentialNotice =
		savedPrimary?.credentialRef && primaryTargetChanged
			? "sync.credentialTargetChanged"
			: credentialNoticeKey(status?.credentialState);
	const changesPendingReview = Boolean(
		pendingConflicts &&
			savedPrimary &&
			(primaryTargetChanged ||
				savedPrimary.conflictPolicy !== form.conflictPolicy ||
				JSON.stringify(savedPrimary.scope) !== JSON.stringify(form.scope)),
	);
	const saveBlocked =
		controlsDisabled ||
		primaryCredentialBlocked ||
		changesPendingReview ||
		(requiresReplacement && !hasNewSecret(form)) ||
		mirrorCredentialsIncomplete;
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

	function reportSyncError(error: unknown) {
		if (isSyncConflictPending(error)) {
			setSyncError(null);
			reviewPending();
			void refresh().catch((error) => notifyError(errorMessage(error)));
			return;
		}
		const detail = errorMessage(error);
		const code = error instanceof Error ? (error as ApiError).code : undefined;
		const message = describeSyncError(detail, code);
		setSyncError(message);
		notifyError(message, {
			id: `sync:${localVault}`,
			...(message === detail ? {} : { description: detail }),
		});
	}

	const save = async () => {
		if (saveBlocked || cloudBlocked) return;
		setSaving(true);
		try {
			const next = await syncConfigure(localVault, form, mirrors);
			requestId.current++;
			setStatus(next);
			setSyncing(next.running);
			setMirrors(
				next.mirrors.map(({ config, credentialState }) =>
					editableConfig(config, credentialState),
				),
			);
			mirrorsDirty.current = false;
			formDirty.current = false;
			if (next.config)
				setForm(editableConfig(next.config, next.credentialState));
			setSyncError(null);
			notifySuccess(t("sync.saved"));
		} catch (error) {
			reportSyncError(error);
		} finally {
			setSaving(false);
		}
	};

	const runSync = async () => {
		if (
			controlsDisabled ||
			primaryCredentialBlocked ||
			credentialNeedsInput ||
			pendingConflicts
		)
			return;
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
			reportSyncError(error);
		} finally {
			setSyncing(false);
			setProgress(null);
			void refresh().catch(reportSyncError);
		}
	};

	const disconnect = async () => {
		if (controlsDisabled || pendingConflicts) return;
		setSaving(true);
		try {
			await syncDisconnect(localVault);
			requestId.current++;
			formDirty.current = false;
			mirrorsDirty.current = false;
			setMirrors([]);
			setMirrorLive({});
			setStatus(null);
			setForm(newConfig());
			setSyncError(null);
			await refresh();
		} catch (error) {
			reportSyncError(error);
		} finally {
			setSaving(false);
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
					disabled={controlsDisabled || Boolean(pendingConflicts)}
				>
					{t("sync.disconnect")}
				</Button>
			</section>
		);

	const configChanged =
		JSON.stringify(form) !== JSON.stringify(status?.config) ||
		(mirrorsDirty.current &&
			JSON.stringify(mirrors) !==
				JSON.stringify(
					savedMirrors.map(({ config, credentialState }) =>
						editableConfig(config, credentialState),
					),
				));
	const canOverwrite =
		!mobile &&
		isWebdav &&
		status?.configured &&
		!configChanged &&
		!pendingConflicts &&
		form.conflictPolicy !== "ask" &&
		!primaryFieldsDisabled &&
		!credentialNeedsInput;

	const openOverwrite = async (direction: SyncOverwriteDirection) => {
		if (!canOverwrite) return;
		try {
			const preview = await syncOverwritePreview(localVault, direction);
			setOverwriteDialog({ direction, preview });
		} catch (error) {
			reportSyncError(error);
		}
	};

	const confirmOverwrite = async () => {
		if (!overwriteDialog || !canOverwrite) return;
		setOverwriting(true);
		try {
			const outcome = await syncOverwrite(
				localVault,
				overwriteDialog.direction,
			);
			setOverwriteDialog(null);
			await onSynced?.();
			setSyncError(null);
			notifySuccess(
				t("sync.overwriteDone", {
					version: outcome.version,
					up: outcome.uploaded,
					down: outcome.downloaded,
					deleted: outcome.deletedLocal,
				}),
			);
		} catch (error) {
			reportSyncError(error);
		} finally {
			setOverwriting(false);
			void refresh().catch(() => undefined);
		}
	};

	const field = (
		key: keyof SyncBackendConfig,
		opts?: { type?: string; placeholder?: string },
	) => (
		<Input
			id={`sync-${key}`}
			className={mobile ? "w-full min-w-0 sm:w-64" : "w-64"}
			disabled={
				primaryFieldsDisabled ||
				Boolean(pendingConflicts && opts?.type !== "password")
			}
			type={opts?.type ?? "text"}
			placeholder={opts?.placeholder}
			value={String(form[key] ?? "")}
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
			: !syncing && pendingConflicts
				? t("sync.statusConflict")
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
					? pendingConflicts
						? "bg-amber-500"
						: "bg-emerald-500"
					: "bg-muted-foreground/35";
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
			disabled={saveBlocked || cloudBlocked}
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
			disabled={
				controlsDisabled ||
				primaryCredentialBlocked ||
				credentialNeedsInput ||
				Boolean(pendingConflicts)
			}
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
			{pendingConflicts && !syncing ? (
				<Button
					type="button"
					variant="outline"
					size="sm"
					className="mb-4"
					onClick={reviewPending}
				>
					{t("sync.conflictReview")}
				</Button>
			) : null}
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
							field: t(secretFieldLabel(form)),
						})}
					</p>
					{primaryCredentialBlocked ? (
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
								disabled={backendLocked || primaryFieldsDisabled}
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
							disabled={backendLocked || primaryFieldsDisabled}
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
				<>
					{status?.configured &&
					!form.conditionalWrites &&
					!form.allowDegradedSync &&
					!degradedNoticeDismissed ? (
						<div className="mb-3 flex items-center gap-2 rounded-xl border border-amber-500/40 bg-amber-500/5 px-3.5 py-2 text-xs">
							<TriangleAlert
								className="size-4 shrink-0 text-amber-600 dark:text-amber-500"
								aria-hidden
							/>
							<span className="min-w-0 flex-1">
								{t("sync.noConditionalWritesShort")}
							</span>
							<button
								type="button"
								className="inline-flex size-5 shrink-0 items-center justify-center rounded-full text-muted-foreground outline-none transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
								aria-label={t("sync.dismissNotice")}
								onClick={() => {
									writeJsonStorage(DEGRADED_NOTICE_DISMISSED_KEY, true);
									setDegradedNoticeDismissed(true);
								}}
							>
								<X className="size-3.5" aria-hidden />
							</button>
						</div>
					) : null}
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
							description={t("sync.webdavPasswordHint")}
							htmlFor="sync-webdavPassword"
						>
							{field("webdavPassword", { type: "password" })}
						</SyncSettingsRow>
						{status?.configured && !form.conditionalWrites ? (
							<SyncSettingsRow
								mobile={mobile}
								label={
									<HelpLabel
										label={t("sync.allowDegradedSync")}
										help={t("sync.degradedSyncRisk")}
									/>
								}
								htmlFor="sync-allowDegradedSync"
							>
								<Switch
									id="sync-allowDegradedSync"
									checked={form.allowDegradedSync}
									onCheckedChange={(value) =>
										patch({ allowDegradedSync: value })
									}
									disabled={primaryFieldsDisabled || Boolean(pendingConflicts)}
								/>
							</SyncSettingsRow>
						) : null}
						{canOverwrite ? (
							<div className="flex flex-wrap items-center gap-2 px-3.5 py-2.5">
								<Button
									type="button"
									variant="outline"
									size="sm"
									disabled={syncing || saving}
									onClick={() => void openOverwrite("remoteToLocal")}
								>
									<ArrowDownToLine data-icon="inline-start" />
									{t("sync.overwritePull")}
								</Button>
								<Button
									type="button"
									variant="outline"
									size="sm"
									disabled={syncing || saving}
									onClick={() => void openOverwrite("localToRemote")}
								>
									<ArrowUpFromLine data-icon="inline-start" />
									{t("sync.overwritePush")}
								</Button>
							</div>
						) : null}
					</SettingsGroup>
				</>
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
							disabled={scopeDisabled}
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
							disabled={scopeDisabled}
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
							disabled={scopeDisabled}
							className={mobile ? "after:-inset-y-3.5" : undefined}
							id="sync-scope-attachments"
							checked={scope.attachments}
							onCheckedChange={(checked) => patchScope("attachments", checked)}
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
								disabled={scopeDisabled}
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
								label={t("sync.sessionToken")}
								htmlFor="sync-sessionToken"
							>
								{field("sessionToken", { type: "password" })}
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
									disabled={scopeDisabled}
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

			<MirrorSection
				mobile={mobile}
				disabled={controlsDisabled}
				mirrors={mirrors}
				statuses={savedMirrors}
				live={mirrorLive}
				onRetry={() => void loadStatus()}
				onChange={(next) => {
					mirrorsDirty.current = true;
					setMirrors(next);
				}}
			/>

			<div className="flex items-center gap-2">
				{status?.configured ? (
					<Button
						type="button"
						size="sm"
						variant="ghost"
						onClick={() => void disconnect()}
						disabled={controlsDisabled || Boolean(pendingConflicts)}
					>
						<Unplug data-icon="inline-start" />
						{t("sync.disconnect")}
					</Button>
				) : null}
			</div>
			{!onReviewConflicts ? (
				<SyncConflictDialog
					mobile={mobile}
					pending={pendingConflicts}
					open={conflictReview.open}
					resolving={conflictReview.resolving}
					onResolve={conflictReview.apply}
					onDismiss={conflictReview.dismiss}
				/>
			) : null}
			{overwriteDialog ? (
				<Dialog
					open
					onOpenChange={(open) => {
						if (!open && !overwriting) setOverwriteDialog(null);
					}}
				>
					<DialogContent className="sm:max-w-md">
						<DialogHeader>
							<DialogTitle>
								{t(
									overwriteDialog.direction === "remoteToLocal"
										? "sync.overwritePullTitle"
										: "sync.overwritePushTitle",
								)}
							</DialogTitle>
							<DialogDescription>
								{overwriteDialog.direction === "remoteToLocal"
									? t("sync.overwritePullBody", {
											delete: overwriteDialog.preview.willDeleteLocal,
											download: overwriteDialog.preview.willDownload,
											version: overwriteDialog.preview.remoteVersion,
										})
									: t("sync.overwritePushBody", {
											upload: overwriteDialog.preview.willUpload,
											remove: overwriteDialog.preview.willRemoveRemote,
											version: overwriteDialog.preview.remoteVersion,
										})}
							</DialogDescription>
						</DialogHeader>
						<DialogFooter>
							<Button
								type="button"
								variant="outline"
								onClick={() => setOverwriteDialog(null)}
								disabled={overwriting}
							>
								{t("sync.cancel")}
							</Button>
							<Button
								type="button"
								variant="destructive"
								onClick={() => void confirmOverwrite()}
								disabled={!canOverwrite}
							>
								{overwriting ? (
									<LoaderCircle
										data-icon="inline-start"
										className="animate-spin"
									/>
								) : null}
								{t("sync.overwriteConfirm")}
							</Button>
						</DialogFooter>
					</DialogContent>
				</Dialog>
			) : null}
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

/** True when two bindings point at the same mirror store (compare like
 * `remote_identity` does: backend + address + account, never secrets). */
function sameSyncTarget(a: SyncBackendConfig, b: SyncBackendConfig): boolean {
	if (a.backend !== b.backend) return false;
	if (a.backend === "webdav") {
		return (
			a.webdavUrl.trim().replace(/\/+$/, "") ===
				b.webdavUrl.trim().replace(/\/+$/, "") &&
			a.webdavUsername.trim() === b.webdavUsername.trim()
		);
	}
	if (a.backend === "git") {
		return (
			a.gitRemote.trim() === b.gitRemote.trim() &&
			a.gitBranch.trim() === b.gitBranch.trim()
		);
	}
	if (a.backend === "s3") {
		return (
			a.endpoint.trim().replace(/\/+$/, "") ===
				b.endpoint.trim().replace(/\/+$/, "") &&
			(a.region.trim() || "us-east-1") === (b.region.trim() || "us-east-1") &&
			a.forcePathStyle === b.forcePathStyle &&
			a.bucket.trim() === b.bucket.trim() &&
			a.prefix.trim().replace(/^\/+|\/+$/g, "") ===
				b.prefix.trim().replace(/^\/+|\/+$/g, "") &&
			a.accessKey.trim() === b.accessKey.trim()
		);
	}
	return (
		a.agenteroApi.trim().replace(/\/+$/, "") ===
			b.agenteroApi.trim().replace(/\/+$/, "") &&
		a.agenteroSubject === b.agenteroSubject &&
		a.agenteroOrgId === b.agenteroOrgId
	);
}

type MirrorLive = Record<number, { status: SyncMirrorPhase; error?: string }>;

/** Read-only mirrors: published after each primary pass, never read back. */
function MirrorSection({
	mobile,
	disabled,
	mirrors,
	statuses,
	live,
	onChange,
	onRetry,
}: {
	mobile: boolean;
	disabled: boolean;
	mirrors: SyncBackendConfig[];
	statuses: SyncMirrorStatus[];
	live: MirrorLive;
	onChange: (next: SyncBackendConfig[]) => void;
	onRetry: () => void;
}) {
	const { t } = useTranslation("settings");
	const patch = (index: number, partial: Partial<SyncBackendConfig>) => {
		const before = mirrors[index];
		const next = { ...before, ...partial };
		// A saved secret belongs to its original destination and account.
		const edited = sameSyncTarget(before, next)
			? next
			: resetSavedCredentials(next);
		onChange(mirrors.map((mirror, i) => (i === index ? edited : mirror)));
	};
	const field = (
		index: number,
		key: keyof SyncBackendConfig,
		label: string,
		options: { type?: "password"; placeholder?: string } = {},
	) => (
		<Input
			type={options.type ?? "text"}
			value={String(mirrors[index][key] ?? "")}
			onChange={(event) => patch(index, { [key]: event.target.value })}
			placeholder={options.placeholder ?? label}
			aria-label={label}
			disabled={disabled}
			autoComplete="off"
			spellCheck={false}
			className={mobile ? undefined : "h-8 text-xs"}
		/>
	);

	return (
		<>
			<SettingsSectionLabel className="mt-4">
				<HelpLabel label={t("sync.mirrors")} help={t("sync.mirrorsHint")} />
			</SettingsSectionLabel>
			<SettingsGroup>
				{mirrors.map((mirror, index) => {
					const savedIndex = statuses.findIndex((entry) =>
						sameSyncTarget(entry.config, mirror),
					);
					const status = statuses[savedIndex];
					const liveState = live[savedIndex];
					const unsupported = mobile && mirror.backend === "git";
					const credentialNotice = credentialNoticeKey(status?.credentialState);
					const error = liveState ? liveState.error : status?.lastError;
					const failure =
						mobile &&
						error === "Download deferred vault assets to publish this mirror"
							? t("sync.mirrorDeferredAssets")
							: error;
					const issue = unsupported
						? t("sync.mirrorUnsupportedBackend")
						: credentialNotice
							? t(credentialNotice, { field: t(secretFieldLabel(mirror)) })
							: failure;
					const dot =
						issue || liveState?.status === "error"
							? "bg-destructive"
							: liveState?.status === "started"
								? "bg-sky-500"
								: liveState?.status === "published" || status?.lastSyncAt
									? "bg-emerald-500"
									: "bg-muted-foreground/35";
					const statusLabel =
						issue || liveState?.status === "error"
							? t("sync.mirrorError")
							: liveState?.status === "started"
								? t("sync.mirrorPublishing")
								: status?.lastSyncAt
									? t("sync.mirrorLastPublished", {
											time: formatLocaleTimestamp(status.lastSyncAt),
										})
									: t("sync.mirrorNeverPublished");
					return (
						<div
							// biome-ignore lint/suspicious/noArrayIndexKey: controlled rows, append/remove only
							key={`${mirror.backend}-${index}`}
							className="space-y-2 border-border/50 border-t px-3 py-2 first:border-t-0"
						>
							<div className="flex flex-wrap items-center gap-2">
								<Select
									value={mirror.backend}
									disabled={disabled || unsupported}
									onValueChange={(value) => {
										if (
											value === "s3" ||
											value === "webdav" ||
											(!mobile && value === "git")
										) {
											patch(index, emptySyncConfig(value));
										}
									}}
								>
									<SelectTrigger
										size="sm"
										className="w-36"
										aria-label={t("sync.mirrorBackend")}
									>
										<SelectValue />
									</SelectTrigger>
									<SelectContent
										className={
											mobile ? "[&_[role=option]]:min-h-11" : undefined
										}
									>
										{BACKEND_OPTIONS.filter(
											({ value }) =>
												value !== "agentero" &&
												(value !== "git" || !mobile || unsupported),
										).map(({ value, labelKey }) => (
											<SelectItem key={value} value={value}>
												{t(labelKey)}
											</SelectItem>
										))}
									</SelectContent>
								</Select>
								<span className="text-muted-foreground text-xs">
									{t("sync.mirrorReadOnly")}
								</span>
								<span className={cn("size-2 rounded-full", dot)} aria-hidden />
								<span
									role="status"
									className="text-muted-foreground text-xs"
									title={issue ?? undefined}
								>
									{statusLabel}
								</span>
								<Button
									type="button"
									size="icon"
									variant="ghost"
									className={cn("ml-auto", mobile && "size-11")}
									aria-label={t("sync.mirrorRemove")}
									disabled={disabled}
									onClick={() =>
										onChange(mirrors.filter((_, i) => i !== index))
									}
								>
									<Trash2 className="size-4" />
								</Button>
							</div>
							{issue ? (
								<p className="wrap-anywhere text-muted-foreground text-xs">
									{issue}
								</p>
							) : null}
							{credentialBlocked(status?.credentialState) ? (
								<Button
									type="button"
									size="sm"
									variant="outline"
									onClick={onRetry}
									disabled={disabled}
								>
									{t("sync.retryCredentials")}
								</Button>
							) : null}
							<div className="grid gap-2 sm:grid-cols-2">
								{mirror.backend === "webdav" ? (
									<>
										{field(index, "webdavUrl", t("sync.serverUrl"), {
											placeholder: "https://dav.example.com/dav/agentero/",
										})}
										{field(index, "webdavUsername", t("sync.username"))}
										{field(index, "webdavPassword", t("sync.password"), {
											type: "password",
										})}
									</>
								) : mirror.backend === "git" ? (
									unsupported ? null : (
										<>
											{field(index, "gitRemote", t("sync.gitRemote"), {
												placeholder: "git@github.com:you/vault.git",
											})}
											{field(index, "gitBranch", t("sync.gitBranch"))}
										</>
									)
								) : (
									<>
										{field(index, "endpoint", t("sync.endpoint"))}
										{field(index, "bucket", t("sync.bucket"))}
										{field(index, "region", t("sync.region"))}
										{field(index, "prefix", t("sync.prefix"))}
										{field(index, "accessKey", t("sync.accessKey"))}
										{field(index, "secretKey", t("sync.secretKey"), {
											type: "password",
										})}
										{field(index, "sessionToken", t("sync.sessionToken"), {
											type: "password",
										})}
										<div className="flex items-center gap-2 text-xs">
											<Switch
												id={`sync-mirror-path-style-${index}`}
												disabled={disabled}
												checked={mirror.forcePathStyle}
												onCheckedChange={(checked) =>
													patch(index, { forcePathStyle: checked })
												}
											/>
											<label htmlFor={`sync-mirror-path-style-${index}`}>
												{t("sync.pathStyle")}
											</label>
										</div>
									</>
								)}
							</div>
						</div>
					);
				})}
				<div className="px-3 py-2">
					<Button
						type="button"
						size="sm"
						variant="outline"
						onClick={() => onChange([...mirrors, emptySyncConfig("webdav")])}
						disabled={disabled || mirrors.length >= 3}
					>
						<Plus data-icon="inline-start" />
						{t("sync.mirrorAdd")}
					</Button>
				</div>
			</SettingsGroup>
		</>
	);
}
