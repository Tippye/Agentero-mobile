// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SyncProgressEvent, SyncStateEvent } from "@/lib/core/bindings";

const host = vi.hoisted(() => {
	const status: SyncStatus = {
		configured: false,
		running: false,
		lastVersion: 0,
		credentialState: "notConfigured",
		mirrors: [],
	};
	const policy: DownloadPolicy = { mode: "full", pinnedPaperIds: [] };
	const availability: PaperAvailability[] = [];
	return {
		status,
		policy,
		availability,
		materialized: false,
		getPolicy: vi.fn(),
		setPolicy: vi.fn(),
		inventory: vi.fn(),
		fetch: vi.fn(),
		evict: vi.fn(),
		scan: vi.fn(),
		localFind: vi.fn(),
		localRead: vi.fn(),
		remoteUrl: vi.fn(),
		onState: null as ((event: { payload: SyncStateEvent }) => void) | null,
		onProgress: null as
			| ((event: { payload: SyncProgressEvent }) => void)
			| null,
		get: vi.fn(),
		configure: vi.fn(),
		disconnect: vi.fn(),
		run: vi.fn(),
		sizes: vi.fn(),
		auth: vi.fn(),
		cloud: vi.fn(),
		error: vi.fn(),
		success: vi.fn(),
		rescan: vi.fn(async () => {}),
	};
});
vi.mock("@/lib/core/tauri", async (original) => ({
	...(await original<typeof import("@/lib/core/tauri")>()),
	isTauri: () => true,
}));
vi.mock("@/lib/core/bindings", () => ({
	commands: {
		syncGetStatus: host.get,
		syncConfigure: host.configure,
		syncDisconnect: host.disconnect,
		syncNow: host.run,
		syncScopeSizes: host.sizes,
		syncCloudUsage: host.cloud,
		syncGetDownloadPolicy: host.getPolicy,
		syncSetDownloadPolicy: host.setPolicy,
		syncListPaperAvailability: host.inventory,
		syncFetchPaper: host.fetch,
		syncEvictPaper: host.evict,
		paperRescan: host.scan,
	},
	events: {
		syncState: {
			listen: async (callback: typeof host.onState) => {
				host.onState = callback;
				return () => {
					host.onState = null;
				};
			},
		},
		syncMirror: { listen: async () => () => {} },
		syncProgress: {
			listen: async (callback: typeof host.onProgress) => {
				host.onProgress = callback;
				return () => {
					host.onProgress = null;
				};
			},
		},
	},
}));
vi.mock("@/features/account/client", () => ({
	authClient: { getSession: host.auth },
}));
vi.mock("@/lib/core/notify", async (importOriginal) => ({
	...(await importOriginal<typeof import("@/lib/core/notify")>()),
	// Keep the real error classification and capture the toasts.
	notifyError: host.error,
	notifySuccess: host.success,
}));

vi.mock("@/lib/paper/media", () => ({
	findLocalPdfPath: host.localFind,
	localFileToArrayBuffer: host.localRead,
	paperRemoteAssetsFromMetadata: host.remoteUrl,
}));

import { useMobilePaperSync } from "@/components/mobile/hooks/use-mobile-paper-sync";
import { MobileDownloadSettings } from "@/components/mobile/mobile-download-settings";
import { MobileLibraryPage } from "@/components/mobile/mobile-library-page";
import { MobileSidebar } from "@/components/mobile/mobile-sidebar";
import { SyncPane } from "@/components/settings/panes/sync-pane";
import { TooltipProvider } from "@/components/ui/tooltip";
import i18n from "@/i18n";
import { loadMobilePaperPdf } from "@/lib/mobile/paper-files";
import {
	fetchMobilePaperIfDeferred,
	setMobilePaperPinned,
} from "@/lib/mobile/paper-sync";
import type { PaperMetadata } from "@/lib/paper/types";
import {
	type DownloadPolicy,
	emptySyncConfig,
	type PaperAvailability,
	type SyncBackendConfig,
	type SyncOutcome,
	type SyncStatus,
} from "@/lib/sync/api";

const vault = "/private/Vault";
const cloudPaper: PaperMetadata = {
	id: "catalog-id-differs",
	path: "papers/example",
	type: "pdf",
	title: "Cloud paper",
	authors: [],
	tags: [],
	status: "completed",
	is_read: false,
	added_at: "2026-11-01",
	updated_at: "2026-11-01",
};
function LibraryWithSync() {
	const sync = useMobilePaperSync({
		vaultPath: vault,
		papers: [cloudPaper],
		active: true,
		onChanged: host.rescan,
	});
	return createElement(MobileLibraryPage, {
		papers: [cloudPaper],
		selected: null,
		loading: false,
		query: "",
		onSelect: vi.fn(),
		syncStates: sync.states,
		onPinChange: sync.setPinned,
		syncProgress: sync.progressText,
	});
}
const outcome: SyncOutcome = {
	version: 2,
	uploaded: 1,
	downloaded: 2,
	deletedLocal: 0,
	removedRemote: 0,
	conflictCopies: [],
};
const reply = (data: unknown) => ({ status: "ok", data: { ok: true, data } });
let root: ReturnType<typeof createRoot>;
let container: HTMLDivElement;
async function render(mobile = true) {
	await act(async () => {
		root.render(
			createElement(
				I18nextProvider,
				{ i18n },
				createElement(
					TooltipProvider,
					null,
					createElement(SyncPane, {
						vaultPath: vault,
						...(mobile ? { mobile } : {}),
						onSynced: host.rescan,
					}),
				),
			),
		);
	});
}
async function click(text: string) {
	const button = [
		...document.querySelectorAll<HTMLButtonElement>("button"),
	].find((element) => element.textContent?.trim() === text);
	expect(button, text).toBeDefined();
	await act(async () => {
		button?.click();
	});
}
async function fill(id: string, value: string) {
	const input = document.getElementById(id) as HTMLInputElement | null;
	expect(input, id).not.toBeNull();
	await act(() => {
		Object.getOwnPropertyDescriptor(
			HTMLInputElement.prototype,
			"value",
		)?.set?.call(input, value);
		input?.dispatchEvent(new Event("input", { bubbles: true }));
	});
}
async function openBackends() {
	await act(() => {
		document
			.getElementById("sync-backend")
			?.dispatchEvent(
				new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
			);
	});
}

beforeEach(async () => {
	vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
	vi.clearAllMocks();
	await i18n.changeLanguage("en");
	host.status = {
		configured: false,
		config: undefined,
		running: false,
		lastSyncAt: undefined,
		lastVersion: 0,
		mirrors: [],
	};
	host.get.mockImplementation(async () => reply(host.status));
	host.configure.mockImplementation(
		async ({ config }: { config: SyncBackendConfig }) => {
			host.status = {
				...host.status,
				configured: true,
				credentialState: "ready",
				config: {
					...config,
					secretKey: config.secretKey ? "***" : "",
					webdavPassword: config.webdavPassword ? "***" : "",
				},
			};
			return reply(host.status);
		},
	);
	host.disconnect.mockImplementation(async () => {
		host.status = { ...host.status, configured: false, config: undefined };
		return reply(null);
	});
	host.run.mockImplementation(async () => reply(outcome));
	host.sizes.mockResolvedValue(
		reply({
			pdf: 2048,
			source: 0,
			attachments: 0,
			ink: 1024,
			translated: 4096,
		}),
	);
	host.policy = { mode: "full", pinnedPaperIds: ["previous"] };
	host.availability = [
		{
			paperId: "example",
			pinned: false,
			assets: [
				{
					path: "papers/example/example.pdf",
					kind: "pdf",
					size: 8,
					materialized: false,
				},
			],
		},
	];
	host.materialized = false;
	host.getPolicy.mockImplementation(async () => reply(host.policy));
	host.setPolicy.mockImplementation(
		async ({ policy }: { policy: DownloadPolicy }) => {
			host.policy = policy;
			return reply(policy);
		},
	);
	host.inventory.mockImplementation(async () =>
		reply(
			host.availability.map((item) => ({
				...item,
				pinned: host.policy.pinnedPaperIds.includes(item.paperId),
			})),
		),
	);
	host.fetch.mockImplementation(async ({ paperId }: { paperId: string }) => {
		host.materialized = true;
		for (const item of host.availability)
			if (item.paperId === paperId)
				for (const asset of item.assets) asset.materialized = true;
		return reply(outcome);
	});
	host.scan.mockResolvedValue({ ok: true, data: { count: 1 } });
	host.localFind.mockImplementation(async () =>
		host.materialized ? "/private/Vault/papers/example/example.pdf" : null,
	);
	host.localRead.mockResolvedValue(new ArrayBuffer(8));
	host.remoteUrl.mockReturnValue({
		pdfUrl: "https://upstream.example.org/paper.pdf",
		htmlUrl: null,
	});
	host.cloud.mockResolvedValue(
		reply({
			signedIn: true,
			used: 0,
			limit: 1024,
			tier: "free",
			planKey: "free",
		}),
	);
	host.auth.mockResolvedValue({
		data: { session: { activeOrganizationId: "" } },
	});
	localStorage.setItem(
		"agentero-recent-vaults",
		JSON.stringify(["/desktop/other-vault"]),
	);
	container = document.createElement("div");
	document.body.appendChild(container);
	root = createRoot(container);
});
afterEach(async () => {
	await act(() => root.unmount());
	container.remove();
	localStorage.clear();
	vi.unstubAllGlobals();
});

describe("tablet composition of mature sync settings", () => {
	it("exposes an accessible settings entry from the tablet menu without requiring pairing", async () => {
		const openSettings = vi.fn();
		const closeMenu = vi.fn();
		await act(async () => {
			root.render(
				createElement(
					I18nextProvider,
					{ i18n },
					createElement(MobileSidebar, {
						open: true,
						status: { paired: false, connected: false },
						onClose: closeMenu,
						onStatus: vi.fn(),
						onPairAnother: vi.fn(),
						onSettings: openSettings,
					}),
				),
			);
		});
		await click("Settings");
		expect(openSettings).toHaveBeenCalledTimes(1);
		expect(closeMenu).toHaveBeenCalledTimes(1);
		expect(host.get).not.toHaveBeenCalled();
		expect(host.auth).not.toHaveBeenCalled();
	});
	it("limits storage choices and native access to the supplied local Vault without desktop account calls", async () => {
		await render();
		expect(host.get).toHaveBeenCalledWith({ vaultPath: vault });
		expect(document.getElementById("sync-vault")).toBeNull();
		await openBackends();
		expect(
			[...document.querySelectorAll('[role="option"]')].map((element) =>
				element.textContent?.trim(),
			),
		).toEqual(["Object storage", "WebDAV"]);
		expect(host.auth).not.toHaveBeenCalled();
		expect(host.cloud).not.toHaveBeenCalled();
	});

	it("offers object storage and WebDAV as read-only mirrors for a connected mobile Vault", async () => {
		host.status = {
			...host.status,
			configured: true,
			credentialState: "ready",
			config: emptySyncConfig("s3"),
		};
		await render();
		await click("Add mirror");
		const backend = document.querySelector('[aria-label="Mirror backend"]');
		expect(backend).not.toBeNull();
		await act(() => {
			backend?.dispatchEvent(
				new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
			);
		});
		expect(
			[...document.querySelectorAll('[role="option"]')]
				.map((element) => element.textContent?.trim())
				.sort(),
		).toEqual(["Object storage", "WebDAV"]);
	});

	it("configures S3 through the existing IPC adapter and preserves masked secrets on later edits", async () => {
		await render();
		await fill("sync-endpoint", "https://s3.example.org");
		await fill("sync-bucket", "research");
		await fill("sync-accessKey", "access-id");
		await fill("sync-secretKey", "secret-value");
		await click("Connect");
		expect(host.configure.mock.calls[0][0]).toMatchObject({
			vaultPath: vault,
			config: {
				backend: "s3",
				endpoint: "https://s3.example.org",
				bucket: "research",
				accessKey: "access-id",
				secretKey: "secret-value",
			},
		});
		expect(
			(document.getElementById("sync-secretKey") as HTMLInputElement).value,
		).toBe("***");
		await fill("sync-endpoint", "https://new.example.org");
		await click("Save");
		expect(host.configure.mock.calls[1][0].config.secretKey).toBe("***");
		expect(host.auth).not.toHaveBeenCalled();
		expect(host.cloud).not.toHaveBeenCalled();
	});

	it("submits the same mature WebDAV fields after switching backend", async () => {
		await render();
		await openBackends();
		const option = [
			...document.querySelectorAll<HTMLElement>('[role="option"]'),
		].find((element) => element.textContent === "WebDAV");
		await act(() => {
			option?.click();
		});
		await fill("sync-webdavUrl", "https://dav.example.org/research");
		await fill("sync-webdavUsername", "reader");
		await fill("sync-webdavPassword", "app-password");
		await click("Connect");
		expect(host.configure.mock.calls[0][0]).toMatchObject({
			vaultPath: vault,
			config: {
				backend: "webdav",
				webdavUrl: "https://dav.example.org/research",
				webdavUsername: "reader",
				webdavPassword: "app-password",
			},
		});
	});

	it("shows only this Vault's valid progress and refreshes the local catalog after a completed manual sync", async () => {
		host.status = {
			...host.status,
			configured: true,
			config: emptySyncConfig("s3"),
		};
		let complete: (value: unknown) => void = () => {};
		host.run.mockImplementationOnce(
			() =>
				new Promise((resolve) => {
					complete = resolve;
				}),
		);
		await render();
		await click("Sync now");
		expect(host.run).toHaveBeenCalledWith({ vaultPath: vault });
		await act(() => {
			host.onProgress?.({
				payload: {
					vaultPath: "/other",
					phase: "download",
					current: 9,
					total: 10,
				},
			});
			host.onProgress?.({
				payload: {
					vaultPath: vault,
					phase: "unexpected",
					current: 1,
					total: 2,
				},
			});
		});
		expect(container.querySelector("progress")).toBeNull();
		await act(() => {
			host.onProgress?.({
				payload: { vaultPath: vault, phase: "download", current: 2, total: 3 },
			});
		});
		expect(container.textContent).toContain("Downloading 2/3");
		expect(container.querySelector("progress")?.value).toBe(2);
		expect(host.rescan).not.toHaveBeenCalled();
		await act(async () => {
			complete(reply(outcome));
		});
		expect(host.rescan).toHaveBeenCalledTimes(1);
		expect(container.querySelector("progress")).toBeNull();
		expect(host.success).toHaveBeenCalledWith(
			"Sync complete: 1 uploaded, 2 downloaded",
		);
	});

	it("reports connection and background sync failures without treating them as success", async () => {
		await render();
		host.configure.mockRejectedValueOnce(new Error("Cannot reach WebDAV"));
		await click("Connect");
		expect(host.error).toHaveBeenCalledWith("Cannot reach WebDAV", {
			id: `sync:${vault}`,
		});
		expect(host.rescan).not.toHaveBeenCalled();
		await act(async () => {
			host.onState?.({
				payload: {
					vaultPath: vault,
					status: "error",
					error: "Storage permission denied",
				},
			});
		});
		expect(host.error).toHaveBeenCalledWith("Storage permission denied", {
			id: `sync:${vault}`,
		});
		expect(host.success).not.toHaveBeenCalled();
	});

	it("retries a failed status read before allowing an existing configuration to be changed", async () => {
		host.get.mockRejectedValueOnce(new Error("Credential storage unavailable"));
		host.status = {
			...host.status,
			configured: true,
			config: {
				...emptySyncConfig("s3"),
				endpoint: "https://existing.example.org",
				secretKey: "***",
			},
		};
		await render();
		expect(host.error).toHaveBeenCalledWith("Credential storage unavailable");
		expect(
			[...container.querySelectorAll("button")].some(
				(button) => button.textContent === "Connect",
			),
		).toBe(false);
		await click("Try again");
		expect(
			(document.getElementById("sync-endpoint") as HTMLInputElement).value,
		).toBe("https://existing.example.org");
		expect(host.configure).not.toHaveBeenCalled();
	});

	it.each([
		"missing",
		"invalid",
	] as const)("recovers %s credentials without losing the configured remote or opaque reference", async (credentialState) => {
		host.status = {
			...host.status,
			configured: true,
			credentialState,
			config: {
				...emptySyncConfig("webdav"),
				credentialRef: "native-reference",
				webdavUrl: "https://dav.example.org/research",
				webdavUsername: "reader",
				webdavPassword: "***",
			},
		};
		await render();
		expect(
			(document.getElementById("sync-webdavUrl") as HTMLInputElement).value,
		).toBe("https://dav.example.org/research");
		expect(
			(document.getElementById("sync-webdavPassword") as HTMLInputElement)
				.value,
		).toBe("");
		await click("Save");
		expect(host.configure).not.toHaveBeenCalled();
		await fill("sync-webdavPassword", "***");
		await click("Save");
		expect(host.configure).not.toHaveBeenCalled();
		await fill("sync-webdavPassword", "replacement-password");
		await click("Save");
		expect(host.configure.mock.calls[0][0]).toMatchObject({
			vaultPath: vault,
			config: {
				credentialRef: "native-reference",
				webdavUrl: "https://dav.example.org/research",
				webdavPassword: "replacement-password",
			},
		});
		expect(container.textContent).not.toContain("Re-enter");
	});

	it.each([
		"locked",
		"unavailable",
	] as const)("blocks writes while credentials are %s, preserves failed disconnects, and retries status", async (credentialState) => {
		host.status = {
			...host.status,
			configured: true,
			credentialState,
			config: {
				...emptySyncConfig("s3"),
				endpoint: "https://existing.example.org",
				secretKey: "***",
			},
		};
		await render();
		await click("Sync now");
		expect(host.run).not.toHaveBeenCalled();
		host.disconnect.mockRejectedValueOnce(
			new Error("Cannot remove saved credentials"),
		);
		await click("Disconnect");
		expect(host.error).toHaveBeenCalledWith("Cannot remove saved credentials", {
			id: `sync:${vault}`,
		});
		expect(
			(document.getElementById("sync-endpoint") as HTMLInputElement).value,
		).toBe("https://existing.example.org");
		host.status = { ...host.status, credentialState: "ready" };
		await click("Check credentials again");
		const syncButton = [
			...container.querySelectorAll<HTMLButtonElement>("button"),
		].find((button) => button.textContent === "Sync now");
		expect(syncButton?.disabled).toBe(false);
		expect(host.configure).not.toHaveBeenCalled();
	});

	it("offers translated PDFs everywhere and ink only on mobile", async () => {
		await render(true);
		expect(document.getElementById("sync-scope-translated")).not.toBeNull();
		expect(document.getElementById("sync-scope-ink")).not.toBeNull();

		await render(false);
		expect(document.getElementById("sync-scope-translated")).not.toBeNull();
		expect(document.getElementById("sync-scope-ink")).toBeNull();
	});

	it("preserves desktop Git and official-account composition when mobile mode is omitted", async () => {
		await render(false);
		await vi.waitFor(async () => {
			await act(async () => {});
			expect(host.auth).toHaveBeenCalled();
		});
		expect(host.cloud).toHaveBeenCalled();
		await openBackends();
		const labels = [...document.querySelectorAll('[role="option"]')].map(
			(element) => element.textContent?.trim(),
		);
		expect(labels).toContain("Git");
		expect(labels).toContain("This account");
	});

	it("requires disconnecting unsupported existing backends without invoking their auth or sync paths", async () => {
		host.status = {
			...host.status,
			configured: true,
			config: emptySyncConfig("agentero"),
		};
		await render();
		expect(container.textContent).toContain(
			"This device supports S3 and WebDAV sync",
		);
		expect(document.getElementById("sync-space")).toBeNull();
		expect(host.auth).not.toHaveBeenCalled();
		expect(host.cloud).not.toHaveBeenCalled();
		await click("Disconnect");
		expect(host.disconnect).toHaveBeenCalledWith({ vaultPath: vault });
		expect(document.getElementById("sync-endpoint")).not.toBeNull();
		expect(host.run).not.toHaveBeenCalled();
	});
});

describe("tablet on-demand downloads", () => {
	it("preserves Full until an explicit popup choice and retains existing pins", async () => {
		await act(async () => {
			root.render(
				createElement(
					I18nextProvider,
					{ i18n },
					createElement(MobileDownloadSettings, { vaultPath: vault }),
				),
			);
		});
		expect(container.textContent).toContain("Download all synced files");
		expect(host.setPolicy).not.toHaveBeenCalled();
		await act(() => {
			document
				.getElementById("mobile-download-mode")
				?.dispatchEvent(
					new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
				);
		});
		const option = [
			...document.querySelectorAll<HTMLElement>('[role="option"]'),
		].find((element) => element.textContent === "Download when opened");
		await act(async () => {
			option?.click();
		});
		expect(host.setPolicy).toHaveBeenCalledWith({
			vaultPath: vault,
			policy: { mode: "onDemand", pinnedPaperIds: ["previous"] },
		});
		expect(host.evict).not.toHaveBeenCalled();
	});

	it("fetches a missing synced PDF once across overlapping reader opens, rescans, then uses local bytes", async () => {
		host.status = {
			...host.status,
			configured: true,
			config: emptySyncConfig("s3"),
		};
		const fetchMissing = vi.fn(() =>
			fetchMobilePaperIfDeferred(vault, cloudPaper),
		);
		const results = await Promise.all([
			loadMobilePaperPdf(vault, cloudPaper, fetchMissing),
			loadMobilePaperPdf(vault, cloudPaper, fetchMissing),
		]);
		for (const result of results) {
			expect(result.bytes?.byteLength).toBe(8);
			expect(result.source).toBeNull();
		}
		expect(host.fetch).toHaveBeenCalledExactlyOnceWith({
			vaultPath: vault,
			paperId: "example",
		});
		expect(host.scan).toHaveBeenCalledWith({ vaultPath: vault });
		expect(host.setPolicy).not.toHaveBeenCalled();
		expect(host.remoteUrl).not.toHaveBeenCalled();
		await loadMobilePaperPdf(vault, cloudPaper, fetchMissing);
		expect(host.fetch).toHaveBeenCalledTimes(1);
		expect(fetchMissing).toHaveBeenCalledTimes(2);
	});

	it("keeps offline/local reading independent of sync and never fabricates a successful failed fetch", async () => {
		host.materialized = true;
		await loadMobilePaperPdf(vault, cloudPaper, () =>
			fetchMobilePaperIfDeferred(vault, cloudPaper),
		);
		expect(host.get).not.toHaveBeenCalled();
		host.materialized = false;
		const upstream = await loadMobilePaperPdf(vault, cloudPaper, () =>
			fetchMobilePaperIfDeferred(vault, cloudPaper),
		);
		expect(upstream.source).toBe("https://upstream.example.org/paper.pdf");
		expect(host.fetch).not.toHaveBeenCalled();
		host.status = {
			...host.status,
			configured: true,
			config: emptySyncConfig("s3"),
		};
		host.fetch.mockRejectedValueOnce(new Error("Offline storage server"));
		await expect(
			loadMobilePaperPdf(vault, cloudPaper, () =>
				fetchMobilePaperIfDeferred(vault, cloudPaper),
			),
		).rejects.toThrow("Offline storage server");
		expect(host.scan).not.toHaveBeenCalled();
		expect(host.setPolicy).not.toHaveBeenCalled();
	});

	it("serializes simultaneous pin changes and unpins without deleting downloaded files", async () => {
		const second = {
			...cloudPaper,
			id: "another-catalog-id",
			path: "papers/second",
		};
		await Promise.all([
			setMobilePaperPinned(vault, cloudPaper, true),
			setMobilePaperPinned(vault, second, true),
		]);
		expect(host.policy).toEqual({
			mode: "full",
			pinnedPaperIds: ["previous", "example", "second"],
		});
		expect(host.fetch).toHaveBeenCalledWith({
			vaultPath: vault,
			paperId: "example",
		});
		await setMobilePaperPinned(vault, cloudPaper, false);
		expect(host.policy.pinnedPaperIds).toEqual(["previous", "second"]);
		expect(host.fetch).toHaveBeenCalledTimes(2);
		expect(host.evict).not.toHaveBeenCalled();
		expect(host.materialized).toBe(true);
	});

	it("exposes per-paper offline intent and refreshes actual inventory after a pin download", async () => {
		host.status = {
			...host.status,
			configured: true,
			credentialState: "ready",
			config: emptySyncConfig("s3"),
		};
		await act(async () => {
			root.render(
				createElement(
					I18nextProvider,
					{ i18n },
					createElement(LibraryWithSync),
				),
			);
		});
		const pin = container.querySelector<HTMLButtonElement>(
			'[aria-label="Keep Cloud paper offline"]',
		);
		expect(pin).not.toBeNull();
		await act(async () => {
			pin?.click();
		});
		await vi.waitFor(async () => {
			await act(async () => {});
			expect(
				container
					.querySelector('[aria-label="Stop keeping Cloud paper offline"]')
					?.getAttribute("aria-pressed"),
			).toBe("true");
		});
		expect(host.fetch).toHaveBeenCalledWith({
			vaultPath: vault,
			paperId: "example",
		});
		expect(
			container.querySelector('[aria-label="PDF available offline"]'),
		).not.toBeNull();
		expect(host.rescan).toHaveBeenCalled();
		expect(host.auth).not.toHaveBeenCalled();
		expect(host.evict).not.toHaveBeenCalled();
	});
});
