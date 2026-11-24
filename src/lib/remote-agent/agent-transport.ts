import { emitTo } from "@tauri-apps/api/event";
import { appCacheDir, join } from "@tauri-apps/api/path";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { lstat, mkdir, remove, writeFile } from "@tauri-apps/plugin-fs";
import type {
	AcpListSessionsResult,
	AcpLoadSessionResult,
	AgentModelsEvent,
	PromptImage,
} from "@/lib/agent/api";
import type {
	ConnectionInfo,
	ElicitationFieldView_Deserialize,
	HostInfo,
	Json,
	Mutation,
	OperationOutcome,
	Question,
	RemoteAttachment,
	RemoteEvent,
	RemoteModelSelection,
	SessionRef,
	SessionSnapshot,
	TaskStatus,
	WorkspaceRef,
} from "@/lib/core/bindings";
import {
	REMOTE_INTERACTION_PREFIX,
	type RemoteElicitationResponse,
	remoteInteractions,
} from "./interactions";
import {
	canUpload,
	ensureWorkspaceKey,
	OperationJournal,
	sessionKey,
	supports,
	validAnswers,
	workspaceKey,
} from "./model";
import { remoteAgent } from "./service";
import {
	RemoteAssistantText,
	remoteHistoryLines,
	remoteToolStatus,
} from "./transcript";

const AGENT_PREFIX = "dsh-remote:";
const UNASSIGNED_WORKSPACE_ID = "agentero:unassigned";
const SESSION_PREFIX = "dsh-remote-session:";
const POLL_INTERVAL = 1200;
const OBSERVATION_TIMEOUT = 120_000;
const MAX_CACHED_PAPER_ATTACHMENTS = 32;

type RemoteContextUsage = { used: number; size: number };

function contextUsage(
	snapshot: SessionSnapshot & { contextUsage?: RemoteContextUsage | null },
) {
	return snapshot.contextUsage ?? null;
}
const imageExtensions = new Map([
	["image/png", "png"],
	["image/jpeg", "jpg"],
	["image/gif", "gif"],
	["image/webp", "webp"],
	["image/bmp", "bmp"],
	["image/avif", "avif"],
]);
const journal = new OperationJournal();
const reservations = new Set<string>();
const runtimeSessions = new Map<string, SessionRef>();

type RemoteRunInput = {
	sessionId?: string;
	prompt: string;
	modelId?: string;
	images?: PromptImage[];
	files?: File[];
	/** The reader's primary PDF. Reusable within the same native session. */
	paperFile?: File;
	title?: string;
};

type RemoteRunAccepted = {
	sessionId: string;
	messageId: string;
	providerSessionId: string;
	agentId: string;
	follow: () => void;
	abandon: () => void;
};

type PendingInteraction = Extract<
	RemoteEvent["payload"],
	{ type: "permissionRequest" | "questionRequest" }
>;
export class RemoteOperationUnconfirmedError extends Error {
	constructor(readonly target: string) {
		super(
			"A remote action may have executed. Check the remote session before continuing.",
		);
		this.name = "RemoteOperationUnconfirmedError";
	}
}

export function isRemoteAgentId(id: string): boolean {
	return id.startsWith(AGENT_PREFIX);
}

function encodedSessionId(session: SessionRef): string {
	return SESSION_PREFIX + encodeURIComponent(sessionKey(session));
}

function emitAgentEvent(
	label: string,
	event: string,
	payload: object,
): Promise<void> {
	return emitTo({ kind: "WebviewWindow", label }, event, payload);
}

function eventKey(event: RemoteEvent): string {
	return JSON.stringify([
		event.sourceStreamId,
		event.sourceId,
		event.cursor?.value,
	]);
}

function modelId(selection: RemoteModelSelection): string {
	return JSON.stringify([selection.provider, selection.model]);
}

function modelSelection(id: string): RemoteModelSelection {
	try {
		const value: unknown = JSON.parse(id);
		if (
			Array.isArray(value) &&
			value.length === 2 &&
			typeof value[0] === "string" &&
			value[0] &&
			typeof value[1] === "string" &&
			value[1]
		)
			return { provider: value[0], model: value[1] };
	} catch {
		// Model IDs come from the provider-aware catalog below.
	}
	throw new Error("Invalid Remote Agent model selection");
}

function busy(status: TaskStatus): boolean {
	return ["running", "waitingForPermission", "waitingForAnswer"].includes(
		status,
	);
}

function terminalStatus(status: TaskStatus): boolean {
	return ["idle", "completed", "failed", "cancelled"].includes(status);
}

function imageData(image: PromptImage, maxBytes: number) {
	const extension = imageExtensions.get(image.mimeType);
	if (!extension)
		throw new Error(`Unsupported image format: ${image.mimeType}`);
	if (image.data.length > Math.ceil(maxBytes / 3) * 4)
		throw new Error("Image exceeds the Remote Agent attachment size limit");
	const data = Uint8Array.from(atob(image.data), (char) => char.charCodeAt(0));
	if (!data.length || data.length > maxBytes)
		throw new Error("Image exceeds the Remote Agent attachment size limit");
	return { name: `image.${extension}`, data };
}

function questionFieldId(index: number): string {
	return `remote-question-${index}`;
}

function questionFields(
	questions: Question[],
): ElicitationFieldView_Deserialize[] {
	return questions.flatMap((question, index) => {
		const id = questionFieldId(index);
		const field: ElicitationFieldView_Deserialize = {
			id,
			title: question.prompt,
			description: null,
			required: true,
			kind: !question.options.length
				? "text"
				: question.multiple
					? "multiselect"
					: "select",
			options: question.options.map((option) => ({
				value: option,
				title: option,
				description: null,
			})),
		};
		if (!question.options.length || !question.allowsCustom) return [field];
		return [
			field,
			{
				id: `${id}__other`,
				title: "",
				description: null,
				required: false,
				kind: "text",
				options: [],
				isOtherAnswer: true,
				parentFieldId: id,
			},
		];
	});
}

function questionAnswers(
	questions: Question[],
	content: RemoteElicitationResponse["content"],
) {
	return questions.map((question, index) => {
		const id = questionFieldId(index);
		const raw = content?.[id];
		const values = Array.isArray(raw) ? raw : raw ? [raw] : [];
		const other = content?.[`${id}__other`];
		return {
			questionId: question.id,
			selected: question.options.length ? values : [],
			custom:
				(question.options.length
					? Array.isArray(other)
						? other.join(", ")
						: other
					: values.join(", ")) || null,
		};
	});
}

export class RemoteAgentTransport {
	readonly agentId: string;
	private workspaceRequest?: Promise<WorkspaceRef>;
	private readonly paperAttachments = new Map<string, RemoteAttachment>();

	constructor(
		private readonly host: HostInfo,
		private readonly connection: ConnectionInfo,
	) {
		const { providerId, connectionId } = connection.connection;
		if (
			providerId !== host.host.connection.providerId ||
			connectionId !== host.host.connection.connectionId
		)
			throw new Error("Remote Agent host belongs to another connection");
		this.agentId =
			AGENT_PREFIX +
			encodeURIComponent(
				JSON.stringify([providerId, connectionId, host.host.hostId]),
			);
	}

	get canUpload(): boolean {
		return this.attachmentLimit > 0;
	}

	get attachmentLimit(): number {
		const capabilities = this.connection.capabilities;
		const limits = capabilities.attachments;
		if (!canUpload(capabilities) || !limits) return 0;
		// The existing Tauri command publishes these files as opaque binary.
		return limits.mimeTypes.length &&
			!limits.mimeTypes.includes("application/octet-stream")
			? 0
			: limits.maxBytes;
	}

	private get paperPath(): string {
		const home = this.host.home?.trim();
		if (!home)
			throw new Error(
				"The Remote Agent host did not report its home directory",
			);
		const separator = /^[A-Za-z]:\\/.test(home) ? "\\" : "/";
		return [home.replace(/[\\/]+$/, ""), "Agentero", "Papers"].join(separator);
	}

	private runtimeKey(id: string): string {
		return JSON.stringify([this.agentId, id]);
	}

	private resolveSession(id: string): SessionRef {
		const mapped = runtimeSessions.get(this.runtimeKey(id));
		if (mapped) return mapped;
		if (id.startsWith(SESSION_PREFIX)) {
			let value: unknown;
			try {
				value = JSON.parse(decodeURIComponent(id.slice(SESSION_PREFIX.length)));
			} catch {
				throw new Error("Invalid Remote Agent session ID");
			}
			const { providerId, connectionId } = this.connection.connection;
			if (
				Array.isArray(value) &&
				value.length === 6 &&
				value[0] === providerId &&
				value[1] === connectionId &&
				value[2] === this.host.host.hostId &&
				typeof value[3] === "string" &&
				value[3] &&
				typeof value[4] === "string" &&
				value[4] &&
				typeof value[5] === "string" &&
				value[5]
			)
				return {
					workspace: {
						host: this.host.host,
						backendId: value[3],
						workspaceId: value[4],
					},
					sessionId: value[5],
				};
		}
		throw new Error("Remote Agent session does not belong to this host");
	}

	private async paperWorkspaces(): Promise<WorkspaceRef[]> {
		const path = this.paperPath;
		const pathKey = (value: string) =>
			/^[A-Za-z]:[\\/]/.test(path)
				? value.replace(/\\/g, "/").replace(/\/$/, "").toLowerCase()
				: value.replace(/\/$/, "");
		return (await remoteAgent.workspaces(this.host.host))
			.filter((row) => row.path && pathKey(row.path) === pathKey(path))
			.map((row) => row.workspace);
	}

	private ensureWorkspace(): Promise<WorkspaceRef> {
		if (!this.workspaceRequest) {
			this.workspaceRequest = (async () => {
				const workspaces = await this.paperWorkspaces();
				const unassigned = workspaces.find(
					(row) => row.workspaceId === UNASSIGNED_WORKSPACE_ID,
				);
				const known = workspaces.find(
					(row) => row.workspaceId !== UNASSIGNED_WORKSPACE_ID,
				);
				if (known) return unassigned ?? known;
				const path = this.paperPath;
				const result = await this.mutate(
					{ type: "ensureWorkspace", host: this.host.host, path },
					ensureWorkspaceKey(this.host.host, path),
				);
				if (result?.type !== "workspaceEnsured")
					throw new Error("Remote Agent did not confirm the paper workspace");
				return unassigned ?? result.workspace;
			})().catch((error) => {
				this.workspaceRequest = undefined;
				throw error;
			});
		}
		return this.workspaceRequest;
	}

	async listSessions(): Promise<AcpListSessionsResult> {
		if (!supports(this.connection.capabilities, "listSessions"))
			return { supported: false, sessions: [] };
		const workspaces = await this.paperWorkspaces();
		const rows = (
			await Promise.all(
				workspaces.map((workspace) => remoteAgent.sessions(workspace)),
			)
		).flat();
		return {
			supported: true,
			sessions: rows.map(({ session, title }) => ({
				sessionId: encodedSessionId(session),
				cwd: this.paperPath,
				title,
			})),
		};
	}

	async loadSession(id: string): Promise<AcpLoadSessionResult> {
		const session = this.resolveSession(id);
		const snapshot = await remoteAgent.read(session);
		return {
			sessionId: encodedSessionId(session),
			title: snapshot.info.title,
			contextUsage: contextUsage(snapshot),
			lines: remoteHistoryLines(snapshot.events),
		};
	}

	async models(sessionId?: string): Promise<AgentModelsEvent | null> {
		if (!supports(this.connection.capabilities, "selectModel")) return null;
		const session =
			sessionId?.startsWith(SESSION_PREFIX) ||
			(sessionId && runtimeSessions.has(this.runtimeKey(sessionId)))
				? this.resolveSession(sessionId)
				: undefined;
		const value = session
			? await remoteAgent.sessionModels(session)
			: await remoteAgent.hostModels(this.host.host);
		if (!value?.routable) return null;
		return {
			sessionId: sessionId ?? "",
			agentId: this.agentId,
			configId: "model",
			currentId:
				value.current.provider && value.current.model
					? modelId(value.current)
					: "",
			models: value.groups.flatMap((group) =>
				group.models.map((model) => ({
					id: modelId({ provider: group.id, model: model.id }),
					name: model.name,
					group: group.name,
				})),
			),
		};
	}

	async selectModel(sessionId: string, id: string): Promise<void> {
		const session = this.resolveSession(sessionId);
		const result = await this.mutate(
			{ type: "selectModel", session, selection: modelSelection(id) },
			sessionKey(session),
		);
		if (result?.type !== "modelSelected")
			throw new Error("Remote Agent did not confirm the model selection");
	}

	private async restoreJournal() {
		const connection = this.connection.connection;
		const records = await remoteAgent.localOperations(connection);
		journal.restore(connection, records);
		return records;
	}

	private async inspectUnknown(
		matches: (id: string, target: string) => boolean,
	) {
		for (const entry of journal.all()) {
			if (
				entry.outcome.status !== "unknown" ||
				!matches(entry.id, entry.target)
			)
				continue;
			const receipt = await remoteAgent
				.receipt(this.connection.connection, entry.id)
				.catch(() => null);
			if (receipt) journal.inspect(entry.id, receipt);
		}
		return journal
			.all()
			.filter(
				({ id, target, outcome }) =>
					outcome.status === "unknown" &&
					!journal.isReviewed(id) &&
					matches(id, target),
			);
	}

	async reviewUnknownOperations(
		target?: string,
	): Promise<{ unknownCount: number; allowNewActions: () => void }> {
		const records = await this.restoreJournal();
		const hostOperations = new Set<string>();
		for (const { operationId, context } of records) {
			if (!context) continue;
			const host =
				context.type === "ensureWorkspace"
					? context.host
					: context.type === "createSession"
						? context.workspace.host
						: context.session.workspace.host;
			if (host.hostId === this.host.host.hostId)
				hostOperations.add(operationId);
		}
		const hostTargetPrefix = `${JSON.stringify([this.connection.connection.providerId, this.connection.connection.connectionId, this.host.host.hostId]).slice(0, -1)},`;
		const unknown = await this.inspectUnknown(
			(id, entryTarget) =>
				(!target || target === entryTarget) &&
				(hostOperations.has(id) || entryTarget.startsWith(hostTargetPrefix)),
		);
		return {
			unknownCount: unknown.length,
			allowNewActions: () => {
				for (const { id } of unknown) journal.allowNewActions(id);
			},
		};
	}

	private async dispatch(
		target: string,
		send: (id: string) => Promise<OperationOutcome>,
	) {
		await this.restoreJournal();
		await this.inspectUnknown((_, entryTarget) => entryTarget === target);
		if (journal.blocked(target))
			throw new RemoteOperationUnconfirmedError(target);
		const id = crypto.randomUUID();
		const outcome = await journal.dispatch(id, target, () => send(id));
		if (outcome.status === "rejected") throw new Error(outcome.message);
		if (outcome.status === "unknown")
			throw new RemoteOperationUnconfirmedError(target);
		return outcome.status === "succeeded" ? outcome.result : undefined;
	}

	private mutate(mutation: Mutation, target: string) {
		return this.dispatch(target, (operationId) =>
			remoteAgent.execute({ operationId, mutation }),
		);
	}

	private async uploadAttachment(
		session: SessionRef,
		name: string,
		data: Uint8Array,
	): Promise<RemoteAttachment> {
		const cache = await appCacheDir();
		if (!cache) throw new Error("Remote Agent attachment cache is unavailable");
		const directory = await join(cache, "remote-agent-attachments");
		await mkdir(directory, { recursive: true });
		const parent = await lstat(directory);
		if (!parent.isDirectory || parent.isSymlink)
			throw new Error("Remote Agent attachment cache is unavailable");
		const extension = name.match(/\.[a-z\d]{1,12}$/i)?.[0] ?? "";
		const stem = name.slice(0, name.length - extension.length);
		let safeName =
			Array.from(stem.replace(/[<>:"/\\|?*\p{Cc}]/gu, "_").trim())
				.slice(0, 48)
				.join("")
				.replace(/[. ]+$/, "") || "attachment";
		if (
			/^(?:con|prn|aux|nul|(?:com|lpt)[1-9])(?:\.|$)|^\.dsh-upload-/i.test(
				safeName,
			)
		)
			safeName = `_${safeName}`;
		const uploadDirectory = await join(directory, crypto.randomUUID());
		await mkdir(uploadDirectory);
		const path = await join(uploadDirectory, `${safeName}${extension}`);
		try {
			await writeFile(path, data, { createNew: true });
			const result = await this.dispatch(sessionKey(session), (id) =>
				remoteAgent.upload(session, path, id),
			);
			if (
				result?.type !== "attachmentUploaded" ||
				sessionKey(result.attachment.session) !== sessionKey(session) ||
				result.attachment.sizeBytes !== data.length
			)
				throw new Error("Remote Agent did not confirm the attachment upload");
			return result.attachment;
		} finally {
			const parent = await lstat(directory).catch(() => null);
			const owned = await lstat(uploadDirectory).catch(() => null);
			if (
				parent?.isDirectory &&
				!parent.isSymlink &&
				owned?.isDirectory &&
				!owned.isSymlink
			) {
				const leaf = await lstat(path).catch(() => null);
				if (leaf?.isFile && !leaf.isSymlink && leaf.size === data.length)
					await remove(path).catch(() => undefined);
				await remove(uploadDirectory).catch(() => undefined);
			}
		}
	}

	private async uploadPaper(session: SessionRef, file: File) {
		const bytes = await file.arrayBuffer();
		const digest = await crypto.subtle?.digest("SHA-256", bytes);
		const key = digest
			? JSON.stringify([sessionKey(session), [...new Uint8Array(digest)]])
			: null;
		const existing = key ? this.paperAttachments.get(key) : undefined;
		if (existing) return existing;

		const uploaded = await this.uploadAttachment(
			session,
			file.name,
			new Uint8Array(bytes),
		);
		if (key) {
			this.paperAttachments.set(key, uploaded);
			if (this.paperAttachments.size > MAX_CACHED_PAPER_ATTACHMENTS) {
				const oldest = this.paperAttachments.keys().next();
				if (!oldest.done) this.paperAttachments.delete(oldest.value);
			}
		}
		return uploaded;
	}

	async run(
		input: RemoteRunInput,
		runtimeSessionId: string,
	): Promise<RemoteRunAccepted> {
		const capabilities = this.connection.capabilities;
		if (
			!supports(capabilities, "sendMessage") ||
			!supports(capabilities, "readSession")
		)
			throw new Error("The Remote Agent host cannot run chat turns");
		const files = input.files ?? [];
		const paperFile = input.paperFile;
		const attachedFiles = paperFile ? [...files, paperFile] : files;
		if ((input.images?.length || attachedFiles.length) && !this.canUpload)
			throw new Error("The Remote Agent host cannot accept attachments");
		const maxBytes = this.attachmentLimit;
		const images =
			input.images?.map((image) => imageData(image, maxBytes)) ?? [];
		if (attachedFiles.some((file) => !file.size || file.size > maxBytes))
			throw new Error(
				"An attachment is empty or exceeds the Remote Agent attachment size limit",
			);
		if (!input.prompt.trim() && !images.length && !attachedFiles.length)
			throw new Error("A Remote Agent turn needs a prompt or an attachment");
		if (input.modelId) modelSelection(input.modelId);
		const runtimeKey = this.runtimeKey(runtimeSessionId);
		let session = input.sessionId
			? this.resolveSession(input.sessionId)
			: runtimeSessions.get(runtimeKey);
		const keys = [runtimeKey];
		const reserve = (key: string) => {
			if (reservations.has(key))
				throw new Error("This Remote Agent session is already running");
			reservations.add(key);
		};
		reserve(runtimeKey);
		const release = () => {
			for (const key of keys) reservations.delete(key);
		};
		try {
			if (!session) {
				const workspace = await this.ensureWorkspace();
				// Native Harness titles are assigned by the Host after the first prompt.
				const result = await this.mutate(
					{ type: "createSession", workspace, title: null },
					workspaceKey(workspace),
				);
				if (result?.type !== "sessionCreated")
					throw new Error("Remote Agent did not confirm the new session");
				session = result.session;
			}
			const key = sessionKey(session);
			reserve(key);
			keys.push(key);
			runtimeSessions.set(runtimeKey, session);
			const baseline = await remoteAgent.read(session);
			if (busy(baseline.info.status))
				throw new Error("This Remote Agent session is already running");
			if (input.modelId) {
				const current = await remoteAgent.sessionModels(session);
				if (!current || modelId(current.current) !== input.modelId)
					await this.selectModel(runtimeSessionId, input.modelId);
			}
			const attachments: RemoteAttachment[] = [];
			const paper = paperFile
				? await this.uploadPaper(session, paperFile)
				: null;
			if (paper) attachments.push(paper);
			for (const image of images)
				attachments.push(
					await this.uploadAttachment(session, image.name, image.data),
				);
			for (const file of files)
				attachments.push(
					await this.uploadAttachment(
						session,
						file.name,
						new Uint8Array(await file.arrayBuffer()),
					),
				);
			const text = paper
				? `${input.prompt}\n\nCurrent paper PDF: ${JSON.stringify(paper.path)} is the uploaded copy in this session's workspace. Read this copy for the paper's full text. The original file path is unavailable on this host.`.trim()
				: input.prompt;
			await this.mutate(
				{ type: "sendMessage", session, text, attachments },
				key,
			);
			const observedSession = session;
			const messageId = crypto.randomUUID();
			let claimed = false;
			const claim = () => {
				if (claimed) return false;
				claimed = true;
				clearTimeout(unclaimed);
				return true;
			};
			const abandon = () => {
				if (claim()) release();
			};
			const unclaimed = setTimeout(abandon, 30_000);
			return {
				sessionId: runtimeSessionId,
				messageId,
				providerSessionId: encodedSessionId(session),
				agentId: this.agentId,
				follow: () => {
					if (!claim()) return;
					void this.followTurn(
						observedSession,
						runtimeSessionId,
						messageId,
						baseline,
					)
						.finally(release)
						.catch(() => undefined);
				},
				abandon,
			};
		} catch (error) {
			release();
			throw error;
		}
	}

	async cancel(sessionId: string): Promise<void> {
		const session = this.resolveSession(sessionId);
		await this.mutate({ type: "cancel", session }, sessionKey(session));
	}

	private async respondPermission(
		sessionId: string,
		payload: Extract<PendingInteraction, { type: "permissionRequest" }>,
		optionId: string | null,
	): Promise<void> {
		const option = optionId ?? "rejected";
		if (!payload.options.includes(option))
			throw new Error("Invalid Remote Agent permission option");
		const session = this.resolveSession(sessionId);
		await this.mutate(
			{
				type: "respondPermission",
				session,
				requestId: payload.requestId,
				optionId: option,
			},
			sessionKey(session),
		);
	}

	private async respondElicitation(
		sessionId: string,
		payload: Extract<PendingInteraction, { type: "questionRequest" }>,
		response: RemoteElicitationResponse,
	): Promise<void> {
		if (response.action !== "accept") return this.cancel(sessionId);
		const session = this.resolveSession(sessionId);
		const { questions, requestId } = payload;
		const answers = questionAnswers(questions, response.content);
		if (!validAnswers(questions, answers))
			throw new Error("Invalid Remote Agent question answers");
		await this.mutate(
			{ type: "respondQuestion", session, requestId, answers },
			sessionKey(session),
		);
	}
	private publishInteraction(
		label: string,
		requestId: string,
		sessionId: string,
		payload: PendingInteraction,
	): Promise<void> {
		if (payload.type === "permissionRequest") {
			remoteInteractions.set(requestId, {
				permission: (optionId) =>
					this.respondPermission(sessionId, payload, optionId),
			});
			return emitAgentEvent(label, "agent:permission-request", {
				requestId,
				sessionId,
				title: payload.description,
				kind: null,
				paths: [],
				host: this.host.name,
				options: payload.options.map((optionId) => ({
					optionId,
					name: "",
					kind:
						optionId === "allowed-once"
							? "allow_once"
							: optionId === "rejected"
								? "reject_once"
								: optionId,
				})),
			});
		}
		remoteInteractions.set(requestId, {
			elicitation: (response) =>
				this.respondElicitation(sessionId, payload, response),
		});
		return emitAgentEvent(label, "agent:elicitation-request", {
			requestId,
			sessionId,
			message: "",
			toolCallId: null,
			fields: questionFields(payload.questions),
		});
	}

	private async followTurn(
		session: SessionRef,
		sessionId: string,
		messageId: string,
		baseline: SessionSnapshot,
	): Promise<void> {
		const { label } = getCurrentWebviewWindow();
		const seen = new Set(baseline.events.map(eventKey));
		const requests = new Set<string>();
		const seenRequests = new Set(
			baseline.events.flatMap(({ payload }) =>
				payload.requestId ? [payload.requestId] : [],
			),
		);
		const answer = new RemoteAssistantText();
		const thought = new RemoteAssistantText();
		let lastProgress = Date.now();
		let readFailures = 0;
		let lastUsage: RemoteContextUsage | null = null;
		let terminal: TaskStatus | undefined;
		const stream = (chunk: string, kind: "message" | "thought") =>
			chunk
				? emitAgentEvent(label, "agent:stream", { sessionId, chunk, kind })
				: Promise.resolve();
		try {
			for (;;) {
				const snapshot = await remoteAgent
					.read(session)
					.catch(async (error) => {
						if (++readFailures >= 3) throw error;
						await emitAgentEvent(label, "agent:status", {
							sessionId,
							phase: "reconnecting",
						});
						return null;
					});
				if (snapshot) {
					readFailures = 0;
					const usage = contextUsage(snapshot);
					if (
						usage &&
						(usage.used !== lastUsage?.used || usage.size !== lastUsage?.size)
					) {
						lastUsage = usage;
						await emitAgentEvent(label, "agent:usage", { sessionId, ...usage });
					}
					const activeRequests = new Set<string>();
					for (const event of snapshot.events) {
						const payload = event.payload;
						if (
							payload.type === "permissionRequest" ||
							payload.type === "questionRequest"
						) {
							const requestId =
								REMOTE_INTERACTION_PREFIX +
								encodeURIComponent(
									JSON.stringify([this.agentId, sessionId, payload.requestId]),
								);
							activeRequests.add(requestId);
							if (!seenRequests.has(payload.requestId)) {
								seenRequests.add(payload.requestId);
								requests.add(requestId);
								await this.publishInteraction(
									label,
									requestId,
									sessionId,
									payload,
								);
							}
							continue;
						}
						const key = eventKey(event);
						if (seen.has(key)) continue;
						seen.add(key);
						lastProgress = Date.now();
						switch (payload.type) {
							case "taskStatus":
								if (terminalStatus(payload.status)) terminal = payload.status;
								break;
							case "assistantChunk":
								await stream(
									thought.chunk(event.sourceId, payload.reasoningDelta ?? ""),
									"thought",
								);
								await stream(
									answer.chunk(event.sourceId, payload.textDelta ?? ""),
									"message",
								);
								break;
							case "message":
								if (payload.role !== "assistant") break;
								await stream(
									thought.message(payload.reasoning ?? ""),
									"thought",
								);
								await stream(answer.message(payload.text), "message");
								break;
							case "tool": {
								const input = payload.arguments
									? parseToolJson(payload.arguments)
									: undefined;
								await emitAgentEvent(label, "agent:tool", {
									sessionId,
									toolCallId: payload.callId,
									title:
										payload.name && payload.name !== "Tool"
											? payload.name
											: undefined,
									status: remoteToolStatus(payload.state),
									input,
									output: payload.output,
								});
								break;
							}
						}
						// Everything after the first new turn/end belongs to a later turn
						// or to transient stream state retained in the snapshot.
						if (terminal) break;
					}
					for (const requestId of requests) {
						if (!activeRequests.has(requestId)) {
							remoteInteractions.delete(requestId);
							requests.delete(requestId);
						}
					}
					if (terminal) {
						if (terminal === "failed")
							throw new Error("The Remote Agent turn failed");
						await emitAgentEvent(label, "agent:completed", {
							sessionId,
							messageId,
							providerSessionId: encodedSessionId(session),
							content: answer.content,
							reasoning: thought.content,
							sources: [],
							stopReason: terminal === "cancelled" ? "cancelled" : null,
						});
						return;
					}
					if (busy(snapshot.info.status)) lastProgress = Date.now();
				}
				if (Date.now() - lastProgress >= OBSERVATION_TIMEOUT)
					throw new Error(
						"Remote Agent turn status is unconfirmed. Reload the session to check its history.",
					);
				await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL));
			}
		} catch (error) {
			await emitAgentEvent(label, "agent:failed", {
				sessionId,
				error: error instanceof Error ? error.message : String(error),
			}).catch(() => undefined);
		} finally {
			for (const requestId of requests) remoteInteractions.delete(requestId);
		}
	}
}

function parseToolJson(raw: string): Json {
	try {
		return JSON.parse(raw) as Json;
	} catch {
		return raw;
	}
}
