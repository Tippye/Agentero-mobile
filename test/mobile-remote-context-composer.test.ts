// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
	Capabilities,
	OperationOutcome,
	OperationRequest,
	RemoteAgentCatalog,
	SessionRef,
} from "@/lib/core/bindings";
import type { MobilePaperRegionContext } from "@/lib/mobile/paper-context";

const state = vi.hoisted(() => ({
	stage: vi.fn(),
	upload: vi.fn(),
	execute: vi.fn(),
	receipt: vi.fn(),
	cleanup: vi.fn(),
	handled: vi.fn(),
	error: vi.fn(),
	open: vi.fn(),
	t: (key: string, options?: Record<string, unknown>) =>
		key === "paperContext.prompt" ? `${key}:${JSON.stringify(options)}` : key,
}));
vi.mock("react-i18next", () => ({ useTranslation: () => ({ t: state.t }) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: state.open }));
vi.mock("@/lib/core/notify", () => ({ notifyError: state.error }));
vi.mock("@/lib/mobile/paper-context", () => ({
	stagePaperRegionImage: state.stage,
}));
vi.mock("@/components/agent/message", () => ({
	Message: ({ children }: { children?: ReactNode }) =>
		createElement("div", null, children),
	MessageContent: ({ children }: { children?: ReactNode }) =>
		createElement("div", null, children),
	MessageResponse: ({ children }: { children?: ReactNode }) =>
		createElement("div", null, children),
}));
vi.mock("@/lib/remote-agent/service", () => ({
	remoteAgent: {
		catalog: async () => catalog(),
		hosts: async () => [],
		localOperations: async () => [],
		read: async (target: SessionRef) => ({
			info: { session: target, title: "Session", status: "idle" },
			events: [],
			cursor: null,
		}),
		upload: state.upload,
		execute: state.execute,
		receipt: state.receipt,
	},
}));

import {
	MobileRemoteAgentPage,
	type MobileRemoteAgentPageProps,
} from "@/components/mobile/remote/mobile-remote-agent-page";

const session: SessionRef = {
	sessionId: "session-a",
	workspace: {
		workspaceId: "workspace",
		backendId: "harness",
		host: {
			hostId: "host",
			connection: {
				providerId: "ds-harness-remote",
				connectionId: "connection",
			},
		},
	},
};
const otherSession: SessionRef = { ...session, sessionId: "session-b" };
const region: MobilePaperRegionContext = {
	id: "11111111-1111-4111-8111-111111111111",
	paperId: "paper-id",
	title: "Paper reference title",
	page: 2,
	region: { x: 0.1, y: 0.1, w: 0.4, h: 0.3 },
	image: { data: "png-preview-fixture", mimeType: "image/png" },
};
let capabilities: Capabilities;
function catalog(): RemoteAgentCatalog {
	return {
		providers: [],
		defaultProvider: "ds-harness-remote",
		persistence: "systemStore",
		credentialState: "ready",
		canClearSavedSignIn: true,
		account: {
			account: "reader@example.test",
			server: "https://remote.example",
			expiresAt: 9999999999999,
			persistence: "systemStore",
			loginProvider: "github",
			info: { connection: session.workspace.host.connection, capabilities },
		},
	};
}
function uploaded(
	target: SessionRef = session,
	path = "uploads/region.png",
): OperationOutcome {
	return {
		status: "succeeded",
		result: {
			type: "attachmentUploaded",
			attachment: { session: target, path, sizeBytes: 100 },
		},
	};
}
function deferred<T>() {
	let finish: (value: T) => void = () => {};
	const promise = new Promise<T>((resolve) => {
		finish = resolve;
	});
	return { promise, resolve: (value: T) => finish(value) };
}
let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
async function render(props: Partial<MobileRemoteAgentPageProps> = {}) {
	await act(async () =>
		root.render(
			createElement(MobileRemoteAgentPage, {
				selection: session,
				onRegionContextHandled: state.handled,
				...props,
			}),
		),
	);
	await act(async () => {});
}
function textarea(): HTMLTextAreaElement {
	const element = container.querySelector("textarea");
	if (!element) throw new Error("Missing composer");
	return element;
}
async function typeQuestion(text: string) {
	await act(async () => {
		const input = textarea();
		Object.getOwnPropertyDescriptor(
			HTMLTextAreaElement.prototype,
			"value",
		)?.set?.call(input, text);
		input.dispatchEvent(new Event("input", { bubbles: true }));
		input.dispatchEvent(new Event("change", { bubbles: true }));
	});
}
function button(label: string): HTMLButtonElement {
	const found = [...container.querySelectorAll("button")].find(
		(item) =>
			item.getAttribute("aria-label") === label || item.textContent === label,
	);
	if (!found) throw new Error(`Missing button ${label}`);
	return found;
}
async function click(label: string) {
	await act(async () => button(label).click());
}
function preview() {
	return container.querySelector('[aria-label="paperContext.preview"]');
}

beforeEach(() => {
	Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
	vi.clearAllMocks();
	sessionStorage.clear();
	capabilities = {
		operations: [
			"listHosts",
			"readSession",
			"sendMessage",
			"uploadAttachment",
			"operationReceipts",
		],
		authentication: ["password"],
		backends: ["harness"],
		attachments: { maxBytes: 1024, mimeTypes: [] },
	};
	state.cleanup.mockResolvedValue(undefined);
	state.stage.mockResolvedValue({
		path: "/private/cache/remote-region-drafts/region.png",
		remove: state.cleanup,
	});
	state.upload.mockImplementation(async (target: SessionRef) =>
		uploaded(target),
	);
	state.execute.mockResolvedValue({ status: "accepted" });
	state.receipt.mockResolvedValue({ status: "notFound" });
	state.open.mockResolvedValue("/private/existing.txt");
	container = document.createElement("div");
	document.body.append(container);
	root = createRoot(container);
});
afterEach(async () => {
	await act(async () => root.unmount());
	container.remove();
});

describe("mobile reviewed region composer", () => {
	it("opens a full region image in the public dialog without staging or changing the question", async () => {
		await render({ regionContext: region });
		await typeQuestion("Inspect this formula before sending");
		await click("paperContext.imageAlt");
		const dialog = document.querySelector('[role="dialog"]');
		expect(dialog).not.toBeNull();
		expect(dialog?.querySelector("img")?.getAttribute("src")).toBe(
			`data:image/png;base64,${region.image.data}`,
		);
		expect(dialog?.textContent).toContain(region.title);
		expect(state.stage).not.toHaveBeenCalled();
		expect(state.upload).not.toHaveBeenCalled();
		expect(state.execute).not.toHaveBeenCalled();
		await act(async () =>
			document.dispatchEvent(
				new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
			),
		);
		expect(document.querySelector('[role="dialog"]')).toBeNull();
		expect(textarea().value).toBe("Inspect this formula before sending");
		expect(preview()).not.toBeNull();
	});

	it("receives and removes a preview without remote mutations or losing typed text and other files", async () => {
		await render();
		await typeQuestion("Keep this typed question");
		state.upload.mockResolvedValueOnce(
			uploaded(session, "uploads/existing.txt"),
		);
		await click("remote.uploadPrivate");
		expect(container.textContent).toContain("uploads/existing.txt");
		await render({ regionContext: region });
		expect(preview()).not.toBeNull();
		expect(textarea().value).toBe("Keep this typed question");
		expect(container.textContent).toContain("uploads/existing.txt");
		expect(state.stage).not.toHaveBeenCalled();
		expect(state.upload).toHaveBeenCalledOnce();
		expect(state.execute).not.toHaveBeenCalled();
		await click("paperContext.remove");
		expect(preview()).toBeNull();
		expect(textarea().value).toBe("Keep this typed question");
		expect(container.textContent).toContain("uploads/existing.txt");
		expect(state.handled).toHaveBeenCalledWith(region.id);
		await render({ regionContext: region });
		expect(preview()).toBeNull();
	});
	it("explicit send uploads then sends, consumes by source ID and clears only the consumed composer", async () => {
		await render({ regionContext: region });
		await typeQuestion("Explain this region");
		await click("remote.send");
		expect(state.stage).toHaveBeenCalledWith(region, 1024);
		expect(state.upload).toHaveBeenCalledOnce();
		expect(state.execute).toHaveBeenCalledOnce();
		expect(state.upload.mock.invocationCallOrder[0]).toBeLessThan(
			state.execute.mock.invocationCallOrder[0],
		);
		const request: OperationRequest = state.execute.mock.calls[0][0];
		if (request.mutation.type !== "sendMessage")
			throw new Error("Expected send");
		expect(request.mutation.text).toContain("Explain this region");
		expect(request.mutation.text).toContain(region.id);
		expect(request.mutation.text).not.toContain("/private/cache");
		expect(state.handled).toHaveBeenCalledOnce();
		expect(state.handled).toHaveBeenCalledWith(region.id);
		expect(preview()).toBeNull();
		expect(textarea().value).toBe("");
		expect(state.cleanup).toHaveBeenCalledOnce();
	});
	it("offers explicit metadata-only fallback rather than silently uploading or dropping the image", async () => {
		capabilities = {
			...capabilities,
			operations: ["readSession", "sendMessage"],
			attachments: null,
		};
		await render({ regionContext: region });
		await typeQuestion("Reference only question");
		expect(container.textContent).toContain("paperContext.referenceOnly");
		expect(button("remote.send").disabled).toBe(true);
		const checkbox = container.querySelector<HTMLInputElement>(
			'input[type="checkbox"]',
		);
		if (!checkbox) throw new Error("Missing include image choice");
		await act(async () => checkbox.click());
		expect(textarea().value).toBe("Reference only question");
		expect(button("remote.send").disabled).toBe(false);
		await click("remote.send");
		expect(state.stage).not.toHaveBeenCalled();
		expect(state.upload).not.toHaveBeenCalled();
		expect(state.execute).toHaveBeenCalledOnce();
	});
	it("retains unknown upload context and question after general review, then uses its receipt without auto-send", async () => {
		state.upload.mockResolvedValueOnce({
			status: "unknown",
			reason: "lost response",
		});
		await render({ regionContext: region });
		await typeQuestion("Retain until confirmed");
		await click("remote.send");
		expect(preview()).not.toBeNull();
		expect(textarea().value).toBe("Retain until confirmed");
		expect(button("remote.send").disabled).toBe(true);
		expect(container.textContent).toContain("paperContext.uncertain");
		await click("remote.continueNewTask");
		expect(textarea().value).toBe("Retain until confirmed");
		expect(button("remote.send").disabled).toBe(true);
		state.receipt.mockResolvedValueOnce({
			status: "found",
			outcome: uploaded(),
		});
		await click("remote.inspectReceipt");
		expect(state.execute).not.toHaveBeenCalled();
		expect(state.upload).toHaveBeenCalledOnce();
		expect(container.textContent).toContain("paperContext.uploaded");
		expect(button("remote.send").disabled).toBe(false);
		await click("remote.send");
		expect(state.upload).toHaveBeenCalledOnce();
		expect(state.execute).toHaveBeenCalledOnce();
		expect(state.handled).toHaveBeenCalledWith(region.id);
	});
	it("fences selection changes during upload and never sends the image to the newly selected session", async () => {
		const upload = deferred<OperationOutcome>();
		state.upload.mockReturnValueOnce(upload.promise);
		await render({ regionContext: region });
		await typeQuestion("Original session question");
		await click("remote.send");
		expect(state.upload).toHaveBeenCalledOnce();
		await render({ selection: otherSession, regionContext: region });
		await act(async () => upload.resolve(uploaded()));
		expect(state.execute).not.toHaveBeenCalled();
		expect(state.handled).not.toHaveBeenCalled();
		expect(preview()).not.toBeNull();
		expect(state.upload.mock.calls[0][0]).toEqual(session);
		expect(state.cleanup).toHaveBeenCalledOnce();
	});
	it("does not consume a current reference when an old-session send response arrives", async () => {
		const response = deferred<OperationOutcome>();
		state.execute.mockReturnValueOnce(response.promise);
		await render({ regionContext: region });
		await typeQuestion("Original task");
		await click("remote.send");
		expect(state.execute).toHaveBeenCalledOnce();
		await render({ selection: otherSession, regionContext: region });
		await act(async () => response.resolve({ status: "accepted" }));
		expect(preview()).not.toBeNull();
		expect(state.handled).not.toHaveBeenCalled();
		const request: OperationRequest = state.execute.mock.calls[0][0];
		if (request.mutation.type !== "sendMessage")
			throw new Error("Expected send");
		expect(request.mutation.session).toEqual(session);
	});
	it("keeps rapid duplicate submit events behind the existing busyRef", async () => {
		const stage = deferred<{ path: string; remove: () => Promise<void> }>();
		state.stage.mockReturnValueOnce(stage.promise);
		await render({ regionContext: region });
		const form = container.querySelector("form");
		if (!form) throw new Error("Missing composer form");
		await act(async () => {
			form.dispatchEvent(
				new Event("submit", { bubbles: true, cancelable: true }),
			);
			form.dispatchEvent(
				new Event("submit", { bubbles: true, cancelable: true }),
			);
		});
		expect(state.stage).toHaveBeenCalledOnce();
		await act(async () =>
			stage.resolve({ path: "/private/cache/one.png", remove: state.cleanup }),
		);
		expect(state.upload).toHaveBeenCalledOnce();
		expect(state.execute).toHaveBeenCalledOnce();
		expect(state.handled).toHaveBeenCalledOnce();
	});
});
