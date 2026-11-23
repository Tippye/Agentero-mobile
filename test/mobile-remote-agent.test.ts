import { describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({
	execute: vi.fn(),
	upload: vi.fn(),
	catalog: vi.fn(),
	logout: vi.fn(),
	localOperations: vi.fn(),
	oauthProviders: vi.fn(),
	oauthStart: vi.fn(),
	oauthPoll: vi.fn(),
	oauthCancel: vi.fn(),
}));
vi.mock("@/lib/core/bindings", () => ({
	commands: {
		remoteAgentExecute: ipc.execute,
		remoteAgentUpload: ipc.upload,
		remoteAgentCatalog: ipc.catalog,
		remoteAgentLogout: ipc.logout,
		remoteAgentLocalOperations: ipc.localOperations,
		remoteAgentOauthProviders: ipc.oauthProviders,
		remoteAgentOauthStart: ipc.oauthStart,
		remoteAgentOauthPoll: ipc.oauthPoll,
		remoteAgentOauthCancel: ipc.oauthCancel,
	},
}));
vi.mock("@/lib/core/tauri", () => ({ isTauri: () => true }));

import type {
	Capabilities,
	JournalEntry,
	Question,
	RemoteAgentCatalog,
	RemoteCredentialState,
	SessionRef,
} from "@/lib/core/bindings";
import {
	canRetrySavedSignIn,
	canSignIn,
	credentialMessageKey,
} from "@/lib/remote-agent/credentials";
import {
	canUpload,
	OperationJournal,
	sessionKey,
	supports,
	validAnswers,
	validTrustConfirmation,
} from "@/lib/remote-agent/model";

import { remoteAgent } from "@/lib/remote-agent/service";

const capabilities: Capabilities = {
	operations: ["listHosts", "sendMessage"],
	authentication: ["browser"],
	backends: ["harness"],
	attachments: null,
};
const session: SessionRef = {
	workspace: {
		host: {
			connection: { providerId: "ds-harness-remote", connectionId: "launch" },
			hostId: "host",
		},
		backendId: "harness",
		workspaceId: "workspace",
	},
	sessionId: "session",
};

describe("native saved sign-in recovery", () => {
	const catalog: RemoteAgentCatalog = {
		providers: [],
		defaultProvider: "ds-harness-remote",
		account: null,
		persistence: "systemStore",
		credentialState: "missing",
		canClearSavedSignIn: false,
	};
	it("permits native login only after the store is known to be empty", () => {
		expect(canSignIn(null)).toBe(false);
		expect(canSignIn(catalog)).toBe(true);
		const failures: RemoteCredentialState[] = [
			"notChecked",
			"locked",
			"unavailable",
			"invalid",
			"reauthenticationRequired",
			"connectionFailed",
		];
		for (const credentialState of failures) {
			expect(
				canSignIn({ ...catalog, credentialState, canClearSavedSignIn: true }),
			).toBe(false);
			expect(credentialMessageKey(credentialState)).not.toBeNull();
		}
		expect(
			canSignIn({
				...catalog,
				persistence: "launchOnly",
				credentialState: "launchOnly",
			}),
		).toBe(true);
		expect(credentialMessageKey("ready")).toBeNull();
	});
	it("keeps failed restore as a usable catalog with an explicit clear action", async () => {
		ipc.catalog.mockResolvedValueOnce({
			status: "ok",
			data: {
				ok: true,
				data: {
					...catalog,
					credentialState: "invalid",
					canClearSavedSignIn: true,
				},
				error: null,
			},
		});
		const restored = await remoteAgent.catalog();
		expect(restored.account).toBeNull();
		expect(restored.canClearSavedSignIn).toBe(true);
		expect(canSignIn(restored)).toBe(false);
		expect(canRetrySavedSignIn(restored.credentialState)).toBe(false);
		expect(canRetrySavedSignIn("locked")).toBe(true);
		expect(canRetrySavedSignIn("connectionFailed")).toBe(true);
		expect(canRetrySavedSignIn("reauthenticationRequired")).toBe(false);
	});
	it("does not report native sign-out success when durable deletion fails", async () => {
		ipc.logout.mockResolvedValueOnce({
			status: "ok",
			data: {
				ok: false,
				data: null,
				error: {
					code: "remote_agent_credentials_locked",
					message: "Unlock the system credential store",
				},
			},
		});
		await expect(remoteAgent.logout()).rejects.toThrow(
			"Unlock the system credential store",
		);
	});
});

describe("third-party browser sign-in", () => {
	const ok = <T>(data: T) => ({
		status: "ok",
		data: { ok: true, data, error: null },
	});
	it("discovers providers, starts a session and completes without a password", async () => {
		ipc.oauthProviders.mockResolvedValueOnce(
			ok([{ id: "github", displayName: "GitHub" }]),
		);
		expect(await remoteAgent.oauthProviders("https://remote.example")).toEqual([
			{ id: "github", displayName: "GitHub" },
		]);
		expect(ipc.oauthProviders).toHaveBeenCalledWith("https://remote.example");
		ipc.oauthStart.mockResolvedValueOnce(
			ok({
				qrId: "github-qr-session-1234567890",
				scanUrl: "https://remote.example/api/v1/auth/q/x",
				expiresIn: 300,
			}),
		);
		const session = await remoteAgent.oauthStart(
			"https://remote.example",
			"github",
		);
		expect(session.qrId).toBe("github-qr-session-1234567890");
		expect(ipc.oauthStart).toHaveBeenCalledWith(
			"https://remote.example",
			"github",
		);
		ipc.oauthPoll.mockResolvedValueOnce(ok({ status: "pending" }));
		expect(await remoteAgent.oauthPoll(session.qrId)).toEqual({
			status: "pending",
		});
		const account = {
			account: "reader@example.test",
			server: "https://remote.example",
			expiresAt: 9_000_000_000_000,
			persistence: "systemStore",
			loginProvider: "github",
			info: {
				connection: { providerId: "ds-harness-remote", connectionId: "launch" },
				capabilities: {
					operations: [],
					authentication: ["browser"],
					backends: [],
					attachments: null,
				},
			},
		};
		ipc.oauthPoll.mockResolvedValueOnce(ok({ status: "complete", account }));
		expect(await remoteAgent.oauthPoll(session.qrId)).toEqual({
			status: "complete",
			account,
		});
		// The account token never crosses IPC: only the activated account view.
		expect(JSON.stringify(ipc.oauthPoll.mock.calls)).not.toContain("token");
	});
	it("surfaces expiry and cancellation as explicit states", async () => {
		ipc.oauthPoll.mockResolvedValueOnce(ok({ status: "expired" }));
		expect(await remoteAgent.oauthPoll("expired-session")).toEqual({
			status: "expired",
		});
		ipc.oauthCancel.mockResolvedValueOnce(ok(null));
		await expect(
			remoteAgent.oauthCancel("expired-session"),
		).resolves.toBeNull();
		ipc.oauthPoll.mockResolvedValueOnce({
			status: "ok",
			data: {
				ok: false,
				data: null,
				error: {
					code: "remote_agent_oauth_unknown_session",
					message: "This sign-in session is no longer active; start again",
				},
			},
		});
		await expect(remoteAgent.oauthPoll("gone")).rejects.toThrow(
			"no longer active",
		);
	});
});

describe("mobile Remote Agent capabilities and identity", () => {
	it("requires negotiated operations and route limits rather than assuming provider support", () => {
		expect(supports(undefined, "sendMessage")).toBe(false);
		expect(supports(capabilities, "respondQuestion")).toBe(false);
		expect(
			canUpload({
				...capabilities,
				attachments: { maxBytes: 20_000, mimeTypes: [] },
			}),
		).toBe(false);
		expect(
			canUpload({ ...capabilities, operations: ["uploadAttachment"] }),
		).toBe(false);
		expect(
			canUpload({
				...capabilities,
				operations: ["uploadAttachment"],
				attachments: { maxBytes: 20_000, mimeTypes: [] },
			}),
		).toBe(true);
	});
	it("keeps backend, provider and connection namespaces in session identity", () => {
		const key = sessionKey(session);
		expect(
			sessionKey({
				...session,
				workspace: { ...session.workspace, backendId: "codex" },
			}),
		).not.toBe(key);
		expect(
			sessionKey({
				...session,
				workspace: {
					...session.workspace,
					host: {
						...session.workspace.host,
						connection: { providerId: "other", connectionId: "launch" },
					},
				},
			}),
		).not.toBe(key);
		expect(
			sessionKey({
				...session,
				workspace: {
					...session.workspace,
					host: {
						...session.workspace.host,
						connection: {
							providerId: "ds-harness-remote",
							connectionId: "other-launch",
						},
					},
				},
			}),
		).not.toBe(key);
	});
	it("requires the complete displayed fingerprint even for a previously pinned Host", () => {
		const identity = {
			host: session.workspace.host,
			name: "Host",
			fingerprint: `SHA256:${"ab".repeat(32)}`,
			pinned: true,
		};
		expect(validTrustConfirmation(identity, "")).toBe(false);
		expect(validTrustConfirmation(identity, "yes")).toBe(false);
		expect(
			validTrustConfirmation(identity, identity.fingerprint.slice(0, 20)),
		).toBe(false);
		expect(validTrustConfirmation(identity, `SHA256:${"cd".repeat(32)}`)).toBe(
			false,
		);
		expect(validTrustConfirmation(identity, identity.fingerprint)).toBe(true);
		expect(
			validTrustConfirmation(
				{ ...identity, fingerprint: "server said okay" },
				"server said okay",
			),
		).toBe(false);
	});
	it("validates structured answers against exactly the advertised options", () => {
		const questions: Question[] = [
			{
				id: "q",
				prompt: "Choose",
				options: ["one", "two"],
				multiple: false,
				allowsCustom: false,
			},
		];
		expect(
			validAnswers(questions, [
				{ questionId: "q", selected: ["one"], custom: null },
			]),
		).toBe(true);
		expect(
			validAnswers(questions, [
				{ questionId: "q", selected: ["one", "two"], custom: null },
			]),
		).toBe(false);
		expect(
			validAnswers(questions, [
				{ questionId: "q", selected: ["unadvertised"], custom: null },
			]),
		).toBe(false);
		expect(
			validAnswers(questions, [
				{ questionId: "q", selected: [], custom: "text" },
			]),
		).toBe(false);
		expect(validAnswers(questions, [])).toBe(false);
		expect(
			validAnswers(questions, [
				{ questionId: "q", selected: [], custom: null },
			]),
		).toBe(false);
		expect(
			validAnswers(
				questions,
				[{ questionId: "q", selected: [], custom: null }],
				["q"],
			),
		).toBe(true);
		expect(
			validAnswers(
				[{ ...questions[0], multiple: true, allowsCustom: true }],
				[{ questionId: "q", selected: ["one", "two"], custom: "detail" }],
			),
		).toBe(true);
	});
});

describe("uncertain Remote Agent writes", () => {
	it("distinguishes a Rust preflight refusal from an interrupted dispatch", async () => {
		ipc.upload.mockResolvedValueOnce({
			status: "ok",
			data: {
				ok: false,
				data: null,
				error: {
					code: "remote_agent_upload_grant",
					message: "Private file required",
				},
			},
		});
		const journal = new OperationJournal();
		expect(
			await journal.dispatch("operation-id-0005", "target", () =>
				remoteAgent.upload(session, "/outside/private", "operation-id-0005"),
			),
		).toEqual({
			status: "rejected",
			code: "remote_agent_upload_grant",
			message: "Private file required",
		});
		expect(journal.blocked("target")).toBe(false);
		ipc.execute.mockRejectedValueOnce(new Error("WebView lost receipt"));
		expect(
			(
				await journal.dispatch("operation-id-0006", "target", () =>
					remoteAgent.execute({
						operationId: "operation-id-0006",
						mutation: { type: "cancel", session },
					}),
				)
			).status,
		).toBe("unknown");
		expect(journal.blocked("target")).toBe(true);
	});
	it("retains only operation IDs and target references across a WebView reload", async () => {
		let saved = "[]";
		const storage = {
			getItem: () => saved,
			setItem: (_key: string, value: string) => {
				saved = value;
			},
		};
		const journal = new OperationJournal(() => {}, storage);
		const send = vi.fn(async () => ({ status: "accepted" as const }));
		await journal.dispatch("operation-id-0004", sessionKey(session), send);
		expect(JSON.parse(saved)).toEqual([
			{ id: "operation-id-0004", target: sessionKey(session) },
		]);
		const restored = new OperationJournal(() => {}, storage);
		expect(restored.blocked(sessionKey(session))).toBe(true);
		await restored.dispatch("operation-id-0004", sessionKey(session), send);
		expect(send).toHaveBeenCalledTimes(1);
		restored.inspect("operation-id-0004", { status: "notFound" });
		expect(restored.blocked(sessionKey(session))).toBe(true);
	});
	it("records uncertainty before dispatch and never retries after an IPC interruption", async () => {
		const journal = new OperationJournal();
		const target = sessionKey(session);
		const send = vi.fn(async () => {
			expect(journal.blocked(target)).toBe(true);
			throw new Error("response lost after remote commit");
		});
		expect(await journal.dispatch("operation-id-0001", target, send)).toEqual({
			status: "unknown",
			reason: "ipcInterrupted",
		});
		await journal.dispatch("operation-id-0001", target, send);
		expect(send).toHaveBeenCalledTimes(1);
		expect(journal.blocked(target)).toBe(true);
		journal.inspect("operation-id-0001", { status: "notFound" });
		expect(journal.blocked(target)).toBe(true);
		journal.allowNewActions("operation-id-0001");
		expect(journal.blocked(target)).toBe(false);
		expect(journal.all()[0].outcome.status).toBe("unknown");
		await journal.dispatch("operation-id-0001", target, send);
		expect(send).toHaveBeenCalledTimes(1);
	});
	it("shares in-flight writes and uses receipts only as observations", async () => {
		const journal = new OperationJournal();
		let finish: ((value: { status: "accepted" }) => void) | undefined;
		const send = vi.fn(
			() =>
				new Promise<{ status: "accepted" }>((resolve) => {
					finish = resolve;
				}),
		);
		const first = journal.dispatch("operation-id-0002", "target", send);
		const second = journal.dispatch("operation-id-0002", "target", send);
		expect(send).toHaveBeenCalledTimes(1);
		finish?.({ status: "accepted" });
		expect(await first).toEqual({ status: "accepted" });
		expect(await second).toEqual({ status: "accepted" });
		journal.inspect("operation-id-0002", {
			status: "found",
			outcome: { status: "succeeded", result: { type: "acknowledged" } },
		});
		expect(journal.all()[0].outcome.status).toBe("succeeded");
		await journal.dispatch("operation-id-0002", "target", send);
		expect(send).toHaveBeenCalledTimes(1);
	});
	it("keeps unknown writes isolated to their namespaced target and clears only on a definitive receipt", async () => {
		const journal = new OperationJournal();
		await journal.dispatch("operation-id-0003", "first", async () => ({
			status: "unknown",
			reason: "transport lost",
		}));
		expect(journal.blocked("first")).toBe(true);
		expect(journal.blocked("other")).toBe(false);
		journal.inspect("operation-id-0003", {
			status: "found",
			outcome: { status: "unknown", reason: "still pending" },
		});
		expect(journal.blocked("first")).toBe(true);
		journal.inspect("operation-id-0003", {
			status: "found",
			outcome: {
				status: "rejected",
				code: "refused",
				message: "Known refusal",
			},
		});
		expect(journal.blocked("first")).toBe(false);
	});
});

describe("native operation history hydration", () => {
	const connection = session.workspace.host.connection;
	const record: JournalEntry = {
		version: 1,
		connection,
		operationId: "native-operation-0001",
		requestDigest: "a".repeat(64),
		context: { type: "sendMessage", session },
		outcome: { status: "unknown", reason: "observation interrupted" },
	};
	it("restores native IDs after WebView storage loss without executing anything", async () => {
		ipc.localOperations.mockResolvedValueOnce({
			status: "ok",
			data: { ok: true, data: [record], error: null },
		});
		const journal = new OperationJournal();
		journal.restore(connection, await remoteAgent.localOperations(connection));
		expect(journal.blocked(sessionKey(session))).toBe(true);
		const send = vi.fn();
		expect(
			await journal.dispatch(record.operationId, sessionKey(session), send),
		).toEqual(record.outcome);
		expect(send).not.toHaveBeenCalled();
	});
	it("accepts a definitive native outcome and never downgrades it on an older observation", () => {
		const journal = new OperationJournal();
		journal.restore(connection, [record]);
		journal.restore(connection, [
			{
				...record,
				outcome: { status: "succeeded", result: { type: "acknowledged" } },
			},
		]);
		journal.restore(connection, [record]);
		expect(journal.blocked(sessionKey(session))).toBe(false);
		expect(journal.all()[0].outcome.status).toBe("succeeded");
	});
	it("keeps a known UI target when a receipt imported without request context is restored", async () => {
		const journal = new OperationJournal();
		journal.restore(connection, [record]);
		journal.restore(connection, [
			{ ...record, requestDigest: null, context: null },
		]);
		expect(journal.blocked(sessionKey(session))).toBe(true);
		const send = vi.fn();
		await journal.dispatch(record.operationId, sessionKey(session), send);
		expect(send).not.toHaveBeenCalled();
	});
	it("rejects foreign connection and target metadata", () => {
		const journal = new OperationJournal();
		expect(() =>
			journal.restore({ ...connection, connectionId: "foreign" }, [record]),
		).toThrow();
		expect(() =>
			journal.restore(connection, [
				{
					...record,
					context: {
						type: "sendMessage",
						session: {
							...session,
							workspace: {
								...session.workspace,
								host: {
									...session.workspace.host,
									connection: { ...connection, connectionId: "foreign" },
								},
							},
						},
					},
				},
			]),
		).toThrow();
		expect(journal.all()).toEqual([]);
	});
});
