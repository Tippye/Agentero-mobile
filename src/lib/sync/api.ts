/**
 * Vault cloud sync (S3-compatible / WebDAV) — Host command wrappers.
 * Host implementation: `src-tauri/src/integration/sync/commands.rs`.
 */

import {
	type ConflictResolution as ConflictResolutionWire,
	commands,
	type DownloadPolicy as DownloadPolicyWire,
	type PaperAssetAvailability as PaperAssetAvailabilityWire,
	type PaperAvailability as PaperAvailabilityWire,
	type PendingConflicts as PendingConflictsWire,
	type SyncBackendConfig as SyncBackendConfigWire,
	type SyncCredentialState as SyncCredentialStateWire,
	type SyncStateEvent_Deserialize,
	type SyncStatus_Serialize,
} from "@/lib/core/bindings";
import { callApiResult } from "@/lib/core/ipc";

/**
 * Which optional paper assets take part in sync. Notes, metadata sidecars,
 * highlights and embedded images always sync — they are small and
 * irreplaceable. Ink and translated PDFs are device-produced artifacts and are
 * opt-in per category.
 */
export type SyncScope = {
	pdf: boolean;
	source: boolean;
	attachments: boolean;
	/** Handwritten ink sidecars (`papers/<id>/marks/ink/`); mobile-only toggle. */
	ink: boolean;
	/** Finished translated PDFs (`papers/<id>/translated/`). */
	translated: boolean;
};

/** Storage backend discriminator; legacy configs without one are S3. */
export type SyncBackendKind = "s3" | "webdav" | "git" | "agentero";

export type SyncBackendConfig = {
	backend: SyncBackendKind;
	/** Opaque native reference; callers cannot use it to read a secret. */
	credentialRef?: string | null;
	endpoint: string;
	region: string;
	bucket: string;
	prefix: string;
	accessKey: string;
	/** Masked (`***`) on the way out; send the mask back to keep the secret. */
	secretKey: string;
	forcePathStyle: boolean;
	/** WebDAV server directory, e.g. `https://dav.jianguoyun.com/dav/agentero/`. */
	webdavUrl: string;
	webdavUsername: string;
	/** Masked like the S3 secret key. */
	webdavPassword: string;
	/** Git remote URL or local path; credentials come from system Git. */
	gitRemote: string;
	/** Branch synced by the Git backend (default `main`). */
	gitBranch: string;
	/** Allow a non-loopback `http://` remote. The token travels in cleartext. */
	gitAllowInsecureHttp: boolean;
	/** Absolute PEM CA path for a self-signed HTTPS remote. Empty uses the system store. */
	gitSslCaInfo: string;
	/** `user` or `organization` for official cloud sync. */
	agenteroSubject: "user" | "organization";
	/** Organization id when `agenteroSubject` is `organization`. */
	agenteroOrgId: string;
	/** API origin. Empty until the host fills the signed-in origin. */
	agenteroApi: string;
	/** Background sync: on open, after 30s quiet, and every intervalMinutes. */
	autoSync: boolean;
	intervalMinutes: number;
	/** Detected on connect: false when the backend rejects conditional PUTs. */
	conditionalWrites: boolean;
	/** Divergent-edit handling: auto (mtime wins) or ask (pause + prompt). */
	conflictPolicy: "auto" | "ask";
	/** Sync scope for bulky paper assets (default: everything). */
	scope: SyncScope;
};

export type SyncCredentialState = SyncCredentialStateWire;
/** How a paused divergent-edit prompt resolves one file. */
export type ConflictResolution = ConflictResolutionWire;
/** Divergent edits a paused pass left for the user to decide. */
export type PendingConflicts = PendingConflictsWire;
export type DownloadPolicy = DownloadPolicyWire;
export type PaperAssetAvailability = PaperAssetAvailabilityWire;
export type PaperAvailability = PaperAvailabilityWire;

export type SyncStatus = {
	configured: boolean;
	credentialState?: SyncCredentialState;
	config?: SyncBackendConfig;
	running: boolean;
	lastSyncAt?: string;
	lastVersion: number;
	/** Divergent edits awaiting a user decision (ask policy). */
	pendingConflicts?: PendingConflicts | null;
};

export type SyncOutcome = {
	version: number;
	uploaded: number;
	downloaded: number;
	deletedLocal: number;
	removedRemote: number;
	conflictCopies: string[];
};

/** `sync:state` payload comes straight from the generated wire contract. */
export type SyncStateEvent = SyncStateEvent_Deserialize;

/** `sync:progress` phases; the wire payload types `phase` as a plain string. */
export type SyncPhase = "scan" | "pull" | "download" | "upload" | "finalize";

export type SyncProgressEvent = {
	vaultPath: string;
	phase: SyncPhase;
	current: number;
	total: number;
};

const SYNC_PHASES: readonly string[] = [
	"scan",
	"pull",
	"download",
	"upload",
	"finalize",
];

export function isSyncProgressPhase(phase: string): phase is SyncPhase {
	return SYNC_PHASES.includes(phase);
}

export const emptySyncConfig = (
	backend: SyncBackendKind = "agentero",
): SyncBackendConfig => ({
	backend,
	endpoint: "",
	region: "us-east-1",
	bucket: "",
	prefix: "",
	accessKey: "",
	secretKey: "",
	forcePathStyle: true,
	webdavUrl: "",
	webdavUsername: "",
	webdavPassword: "",
	gitRemote: "",
	gitBranch: "main",
	gitAllowInsecureHttp: false,
	gitSslCaInfo: "",
	agenteroSubject: "user",
	agenteroOrgId: "",
	agenteroApi: "",
	autoSync: true,
	intervalMinutes: 30,
	conditionalWrites: true,
	// Desktop and CLI keep the historical automatic policy; mobile asks.
	conflictPolicy: "auto",
	// Product defaults: ink has always synced; translated PDFs are opt-in.
	scope: {
		pdf: true,
		source: true,
		attachments: true,
		ink: true,
		translated: false,
	},
});

/** Interval choices shared with the Host (`config::INTERVAL_CHOICES`). */
export const SYNC_INTERVAL_CHOICES = [15, 30, 60];

/** Local disk usage per bulky-asset category (bytes). */
export type SyncScopeSizes = {
	pdf: number;
	source: number;
	attachments: number;
	ink: number;
	translated: number;
};

/** Fold the generated wire status into the local all-fields-present shape. */
function statusFromWire(status: SyncStatus_Serialize): SyncStatus {
	const config = status.config;
	return {
		configured: status.configured,
		credentialState: status.credentialState,
		config: config ? configFromWire(config) : undefined,
		running: status.running,
		lastSyncAt: status.lastSyncAt ?? undefined,
		lastVersion: status.lastVersion,
	};
}

function configFromWire(config: SyncBackendConfigWire): SyncBackendConfig {
	return {
		backend: (config.backend as SyncBackendKind | undefined) ?? "s3",
		credentialRef: config.credentialRef ?? null,
		endpoint: config.endpoint ?? "",
		region: config.region ?? "us-east-1",
		bucket: config.bucket ?? "",
		prefix: config.prefix ?? "",
		accessKey: config.accessKey ?? "",
		secretKey: config.secretKey ?? "",
		forcePathStyle: config.forcePathStyle ?? true,
		webdavUrl: config.webdavUrl ?? "",
		webdavUsername: config.webdavUsername ?? "",
		webdavPassword: config.webdavPassword ?? "",
		gitRemote: config.gitRemote ?? "",
		gitBranch: config.gitBranch ?? "main",
		gitAllowInsecureHttp: config.gitAllowInsecureHttp ?? false,
		gitSslCaInfo: config.gitSslCaInfo ?? "",
		agenteroSubject:
			config.agenteroSubject === "organization" ? "organization" : "user",
		agenteroOrgId: config.agenteroOrgId ?? "",
		agenteroApi: config.agenteroApi ?? "",
		autoSync: config.autoSync ?? true,
		intervalMinutes: config.intervalMinutes ?? 30,
		conditionalWrites: config.conditionalWrites ?? true,
		conflictPolicy: config.conflictPolicy === "ask" ? "ask" : "auto",
		scope: {
			pdf: config.scope?.pdf ?? true,
			source: config.scope?.source ?? true,
			attachments: config.scope?.attachments ?? true,
			ink: config.scope?.ink ?? true,
			translated: config.scope?.translated ?? false,
		},
	};
}

export async function syncGetStatus(vaultPath: string): Promise<SyncStatus> {
	return statusFromWire(
		await callApiResult(() => commands.syncGetStatus({ vaultPath })),
	);
}

export function syncScopeSizes(vaultPath: string): Promise<SyncScopeSizes> {
	return callApiResult(() => commands.syncScopeSizes({ vaultPath }));
}

export async function syncConfigure(
	vaultPath: string,
	config: SyncBackendConfig,
): Promise<SyncStatus> {
	return statusFromWire(
		await callApiResult(() => commands.syncConfigure({ vaultPath, config })),
	);
}

export async function syncDisconnect(vaultPath: string): Promise<void> {
	await callApiResult(() => commands.syncDisconnect({ vaultPath }));
}

export async function syncResolveConflicts(
	vaultPath: string,
	resolutions: Record<string, ConflictResolution>,
	defaultResolution: ConflictResolution = "keepBoth",
): Promise<SyncOutcome> {
	const res = await callApiResult(() =>
		commands.syncResolveConflicts({
			vaultPath,
			defaultResolution,
			resolutions,
		}),
	);
	return res;
}

export function syncNow(vaultPath: string): Promise<SyncOutcome> {
	return callApiResult(() => commands.syncNow({ vaultPath }));
}

export function syncGetDownloadPolicy(
	vaultPath: string,
): Promise<DownloadPolicy> {
	return callApiResult(() => commands.syncGetDownloadPolicy({ vaultPath }));
}
export function syncSetDownloadPolicy(
	vaultPath: string,
	policy: DownloadPolicy,
): Promise<DownloadPolicy> {
	return callApiResult(() =>
		commands.syncSetDownloadPolicy({ vaultPath, policy }),
	);
}
export function syncListPaperAvailability(
	vaultPath: string,
): Promise<PaperAvailability[]> {
	return callApiResult(() => commands.syncListPaperAvailability({ vaultPath }));
}
export function syncFetchPaper(
	vaultPath: string,
	paperId: string,
): Promise<SyncOutcome> {
	return callApiResult(() => commands.syncFetchPaper({ vaultPath, paperId }));
}
export function syncEvictPaper(
	vaultPath: string,
	paperId: string,
): Promise<number> {
	return callApiResult(() => commands.syncEvictPaper({ vaultPath, paperId }));
}
export async function syncNotifyChange(vaultPath: string): Promise<void> {
	await callApiResult(() => commands.syncNotifyChange({ vaultPath }));
}

export type SyncCloudUsage = {
	signedIn: boolean;
	used: number;
	limit: number;
	tier: string;
	planKey: string;
};

export function syncCloudUsage(
	subject: "user" | "organization",
	orgId?: string,
): Promise<SyncCloudUsage> {
	return callApiResult(() =>
		commands.syncCloudUsage({ subject, orgId: orgId ?? null }),
	);
}
