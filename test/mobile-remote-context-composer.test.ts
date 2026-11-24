import { readFile } from "node:fs/promises";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
	ConnectionInfo,
	HostInfo,
	OperationRequest,
	SessionRef,
	SessionSnapshot,
} from "@/lib/core/bindings";
import {
	RemoteAgentTransport,
	RemoteOperationUnconfirmedError,
} from "@/lib/remote-agent/agent-transport";

const io = vi.hoisted(() => ({
	mkdir: vi.fn(),
	write: vi.fn(),
	remove: vi.fn(),
	read: vi.fn(),
	workspaces: vi.fn(),
	sessions: vi.fn(),
	upload: vi.fn(),
	execute: vi.fn(),
	receipt: vi.fn(),
}));

vi.mock("@tauri-apps/api/path", () => ({
	appCacheDir: async () => "/private/cache",
	join: async (...parts: string[]) => parts.join("/"),
}));
vi.mock("@tauri-apps/plugin-fs", () => ({
	mkdir: io.mkdir,
	writeFile: io.write,
	remove: io.remove,
	lstat: async (path: string) => {
		const directory = io.mkdir.mock.calls.some(([created]) => created === path);
		return {
			isDirectory: directory,
			isFile: !directory,
			size: 3,
			isSymlink: false,
		};
	},
}));
vi.mock("@/lib/remote-agent/service", () => ({
	remoteAgent: {
		workspaces: io.workspaces,
		sessions: io.sessions,
		read: io.read,
		localOperations: async () => [],
		upload: io.upload,
		execute: io.execute,
		receipt: io.receipt,
	},
}));

const image = { data: "YWJj", mimeType: "image/png" };
const prompt = "Explain the region on page 2";
let sequence = 0;
let transport: RemoteAgentTransport;
let session: SessionRef;
let otherSession: SessionRef;

function snapshot(
	target: SessionRef,
	status: SessionSnapshot["info"]["status"] = "idle",
): SessionSnapshot {
	return {
		info: { session: target, title: "Paper discussion", status },
		events: [],
		cursor: null,
	};
}

async function sessionIds() {
	const history = await transport.listSessions();
	return history.sessions.map((item) => item.sessionId);
}

beforeEach(() => {
	vi.resetAllMocks();
	const connection: ConnectionInfo = {
		connection: {
			providerId: "ds-harness-remote",
			connectionId: `region-test-${++sequence}`,
		},
		capabilities: {
			operations: [
				"listWorkspaces",
				"listSessions",
				"readSession",
				"createSession",
				"sendMessage",
				"uploadAttachment",
				"operationReceipts",
			],
			authentication: ["password"],
			backends: ["harness"],
			attachments: { maxBytes: 1024, mimeTypes: [] },
		},
	};
	const host: HostInfo = {
		host: { hostId: "host", connection: connection.connection },
		name: "Research host",
		home: "/home/researcher",
		online: true,
		pinned: true,
	};
	const workspace = {
		host: host.host,
		backendId: "harness",
		workspaceId: "papers",
	};
	session = { workspace, sessionId: "session-a" };
	otherSession = { workspace, sessionId: "session-b" };
	io.workspaces.mockResolvedValue([
		{ workspace, name: "Papers", path: "/home/researcher/Agentero/Papers" },
	]);
	io.sessions.mockResolvedValue([
		snapshot(session).info,
		snapshot(otherSession).info,
	]);
	io.read.mockImplementation(async (target: SessionRef) => snapshot(target));
	io.write.mockResolvedValue(undefined);
	io.remove.mockResolvedValue(undefined);
	io.upload.mockImplementation(async (target: SessionRef) => ({
		status: "succeeded",
		result: {
			type: "attachmentUploaded",
			attachment: { session: target, path: "uploads/image.png", sizeBytes: 3 },
		},
	}));
	io.execute.mockResolvedValue({ status: "accepted" });
	io.receipt.mockResolvedValue({ status: "notFound" });
	transport = new RemoteAgentTransport(host, connection);
});

describe("shared Remote Agent region attachment transport", () => {
	it("loads the selected session without staging or transferring an attachment", async () => {
		const [id] = await sessionIds();
		const contextUsage = { used: 1500, size: 32000 };
		io.read.mockResolvedValueOnce({ ...snapshot(session), contextUsage });
		expect(await transport.loadSession(id)).toMatchObject({
			sessionId: id,
			title: "Paper discussion",
			contextUsage,
		});
		expect(io.read).toHaveBeenCalledWith(session);
		expect(io.write).not.toHaveBeenCalled();
		expect(io.upload).not.toHaveBeenCalled();
		expect(io.execute).not.toHaveBeenCalled();
	});

	it("starts a v3 session in the unassigned scope after provisioning the Papers directory", async () => {
		const papers = session.workspace;
		const unassigned = { ...papers, workspaceId: "agentero:unassigned" };
		const unassignedSession = {
			workspace: unassigned,
			sessionId: "new-papers-session",
		};
		io.workspaces.mockResolvedValue([
			{
				workspace: unassigned,
				name: "Papers",
				path: "/home/researcher/Agentero/Papers",
			},
		]);
		io.execute
			.mockResolvedValueOnce({
				status: "succeeded",
				result: { type: "workspaceEnsured", workspace: papers },
			})
			.mockResolvedValueOnce({
				status: "succeeded",
				result: { type: "sessionCreated", session: unassignedSession },
			});
		const webPrompt = `${prompt}. See https://example.org/article.pdf#page=2`;
		const accepted = await transport.run(
			{ prompt: webPrompt, paperId: "no-upload" },
			"unassigned-runtime",
		);
		accepted.abandon();
		expect(
			io.execute.mock.calls.map(([request]) => request.mutation),
		).toMatchObject([
			{ type: "ensureWorkspace", path: "/home/researcher/Agentero/Papers" },
			{ type: "createSession", workspace: unassigned },
			{ type: "sendMessage", session: unassignedSession, text: webPrompt },
		]);
	});

	it("checks the session baseline before staging any attachment", async () => {
		const [id] = await sessionIds();
		io.read.mockResolvedValueOnce(snapshot(session, "running"));
		await expect(
			transport.run({ sessionId: id, prompt, images: [image] }, "runtime-a"),
		).rejects.toThrow("already running");
		expect(io.read).toHaveBeenCalledWith(session);
		expect(io.write).not.toHaveBeenCalled();
		expect(io.upload).not.toHaveBeenCalled();
		expect(io.execute).not.toHaveBeenCalled();
	});

	it("uploads the region, removes its staged file, then sends only the confirmed attachment", async () => {
		const [id] = await sessionIds();
		const accepted = await transport.run(
			{ sessionId: id, prompt, images: [image] },
			"runtime-a",
		);
		accepted.abandon();
		const [target, path] = io.upload.mock.calls[0];
		expect(target).toEqual(session);
		expect(path).toMatch(/^\/private\/cache\/remote-agent-attachments\//);
		expect(io.write).toHaveBeenCalledExactlyOnceWith(
			path,
			new Uint8Array([97, 98, 99]),
			{ createNew: true },
		);
		expect(io.remove.mock.calls).toEqual([
			[path],
			[path.slice(0, path.lastIndexOf("/"))],
		]);
		expect(io.read.mock.invocationCallOrder[0]).toBeLessThan(
			io.upload.mock.invocationCallOrder[0],
		);
		expect(io.upload.mock.invocationCallOrder[0]).toBeLessThan(
			io.execute.mock.invocationCallOrder[0],
		);
		const request: OperationRequest = io.execute.mock.calls[0][0];
		expect(request.mutation).toEqual({
			type: "sendMessage",
			session,
			text: prompt,
			attachments: [{ session, path: "uploads/image.png", sizeBytes: 3 }],
		});
	});

	it("uses an actual uploaded paper alongside manual files and regions and reuses it only within the same session", async () => {
		const [id, otherId] = await sessionIds();
		const bytes = Uint8Array.from(
			await readFile(new URL("./fixtures/rotate90.pdf", import.meta.url)),
		);
		const paperFile = new File([bytes], "Fixture article.pdf", {
			type: "application/pdf",
		});
		const manual = new File(["abc"], "Notes.txt", { type: "text/plain" });
		const paperId = "shelf/paper (α)#%";
		const webUrl = "https://example.org/article.pdf#page=2";
		const citation =
			"[p. N](<./agentero-paper/shelf%2Fpaper%20(%CE%B1)%23%25.pdf#page=N>)";
		const receiptPath = "./Fixture article (1)'s copy.pdf";
		io.upload.mockImplementation(async (target: SessionRef, path: string) => {
			const data = io.write.mock.calls.find(
				([filePath]) => filePath === path,
			)?.[1] as Uint8Array;
			return {
				status: "succeeded",
				result: {
					type: "attachmentUploaded",
					attachment: {
						session: target,
						path: path.endsWith("/Fixture article.pdf")
							? receiptPath
							: `./${path.split("/").at(-1)}`,
						sizeBytes: data.byteLength,
					},
				},
			};
		});

		const first = await transport.run(
			{
				sessionId: id,
				prompt: `${prompt}. Compare it with ${webUrl}`,
				paperFile,
				paperId,
				images: [image],
				files: [manual],
			},
			"paper-first",
		);
		first.abandon();
		expect(io.write.mock.calls[0]?.[1]).toEqual(bytes);
		const firstRequest: OperationRequest = io.execute.mock.calls[0][0];
		expect(firstRequest.mutation).toMatchObject({
			type: "sendMessage",
			attachments: [
				{ session, path: receiptPath, sizeBytes: bytes.byteLength },
				{ session, sizeBytes: 3 },
				{ session, path: "./Notes.txt", sizeBytes: 3 },
			],
		});
		if (firstRequest.mutation.type !== "sendMessage")
			throw new Error("Expected sendMessage");
		const { text } = firstRequest.mutation;
		expect(text).toContain(webUrl);
		expect(text).toContain(
			`${JSON.stringify(receiptPath)} is the uploaded copy`,
		);
		expect(text).toContain(citation);
		expect(text).toContain(
			"Cite the supporting page inline immediately after each claim",
		);
		expect(text).toContain("1-based physical PDF page number");
		expect(text).toContain(
			"Never invent page numbers, citations, or references",
		);
		expect(text).toContain("ordinary HTTP(S) links for verified web sources");
		expect(text).not.toContain("/private/cache");

		const second = await transport.run(
			{ sessionId: id, prompt: "What about Figure 1?", paperFile, paperId },
			"paper-followup",
		);
		second.abandon();
		expect(io.upload).toHaveBeenCalledTimes(3);
		expect(io.execute.mock.calls[1][0].mutation).toMatchObject({
			text: expect.stringContaining(citation),
			attachments: [
				{ session, path: receiptPath, sizeBytes: bytes.byteLength },
			],
		});

		const duplicate = await transport.run(
			{ sessionId: id, prompt, paperFile, paperId: "duplicate-paper" },
			"paper-duplicate",
		);
		duplicate.abandon();
		expect(io.upload).toHaveBeenCalledTimes(3);
		const duplicateText = io.execute.mock.calls[2][0].mutation.text;
		expect(duplicateText).toContain(
			"./agentero-paper/duplicate-paper.pdf#page=N",
		);
		expect(duplicateText).not.toContain(citation);

		const other = await transport.run(
			{ sessionId: otherId, prompt, paperFile },
			"paper-other",
		);
		other.abandon();
		expect(io.upload).toHaveBeenCalledTimes(4);
		expect(io.upload.mock.calls[3][0]).toEqual(otherSession);
		const otherText = io.execute.mock.calls[3][0].mutation.text;
		expect(otherText).toContain(
			`${JSON.stringify(receiptPath)} is the uploaded copy`,
		);
		expect(otherText).not.toContain("agentero-paper/");
	});

	it("does not send when the upload receipt belongs to another session", async () => {
		const [id] = await sessionIds();
		io.upload.mockResolvedValueOnce({
			status: "succeeded",
			result: {
				type: "attachmentUploaded",
				attachment: {
					session: otherSession,
					path: "uploads/other.png",
					sizeBytes: 3,
				},
			},
		});
		await expect(
			transport.run({ sessionId: id, prompt, images: [image] }, "runtime-a"),
		).rejects.toThrow("did not confirm the attachment upload");
		const [, path] = io.upload.mock.calls[0];
		expect(io.remove.mock.calls).toEqual([
			[path],
			[path.slice(0, path.lastIndexOf("/"))],
		]);
		expect(io.execute).not.toHaveBeenCalled();
	});

	it("blocks a repeat after an unknown upload and keeps the other session independent", async () => {
		const [id, otherId] = await sessionIds();
		io.upload.mockResolvedValueOnce({
			status: "unknown",
			reason: "response lost",
		});
		const input = { sessionId: id, prompt, images: [image] };
		await expect(transport.run(input, "runtime-a")).rejects.toThrow(
			"may have executed",
		);
		await expect(transport.run(input, "runtime-a")).rejects.toThrow(
			RemoteOperationUnconfirmedError,
		);
		expect(io.receipt).toHaveBeenCalledOnce();
		expect(io.upload).toHaveBeenCalledOnce();
		expect(io.execute).not.toHaveBeenCalled();

		const accepted = await transport.run(
			{ sessionId: otherId, prompt: "Separate discussion" },
			"runtime-b",
		);
		accepted.abandon();
		const request: OperationRequest = io.execute.mock.calls[0][0];
		expect(request.mutation).toEqual({
			type: "sendMessage",
			session: otherSession,
			text: "Separate discussion",
			attachments: [],
		});
	});
});
