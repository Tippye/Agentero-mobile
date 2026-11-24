import type {
	Capabilities,
	Capability,
	ConnectionRef,
	JournalEntry,
	OperationOutcome,
	Question,
	QuestionAnswer,
	ReceiptLookup,
	SessionRef,
	WorkspaceRef,
} from "@/lib/core/bindings";

export function supports(
	capabilities: Capabilities | undefined,
	operation: Capability,
): boolean {
	return capabilities?.operations.includes(operation) ?? false;
}
export function canUpload(capabilities: Capabilities | undefined): boolean {
	return (
		supports(capabilities, "uploadAttachment") &&
		(capabilities?.attachments?.maxBytes ?? 0) > 0
	);
}
export function workspaceKey(workspace: WorkspaceRef): string {
	return JSON.stringify([
		workspace.host.connection.providerId,
		workspace.host.connection.connectionId,
		workspace.host.hostId,
		workspace.backendId,
		workspace.workspaceId,
	]);
}
export function sessionKey(session: SessionRef): string {
	return `${workspaceKey(session.workspace).slice(0, -1)},${JSON.stringify(session.sessionId)}]`;
}
export function belongsToConnection(
	target: string,
	connection: ConnectionRef,
): boolean {
	return target.startsWith(
		`${JSON.stringify([connection.providerId, connection.connectionId]).slice(0, -1)},`,
	);
}
export function validAnswers(
	questions: Question[],
	answers: QuestionAnswer[],
	skipped: string[] = [],
): boolean {
	return (
		answers.length === questions.length &&
		questions.every((question) => {
			const matching = answers.filter(
				(answer) => answer.questionId === question.id,
			);
			if (matching.length !== 1) return false;
			const answer = matching[0];
			return (
				(question.multiple || answer.selected.length <= 1) &&
				new Set(answer.selected).size === answer.selected.length &&
				answer.selected.every((option) => question.options.includes(option)) &&
				(!answer.custom?.trim() || question.allowsCustom) &&
				(answer.selected.length > 0 ||
					Boolean(answer.custom?.trim()) ||
					skipped.includes(question.id))
			);
		})
	);
}

export type OperationEntry = {
	id: string;
	target: string;
	outcome: OperationOutcome;
};

/** The ID is recorded before dispatch. An IPC rejection may follow a successful remote write. */
export class OperationJournal {
	private entries = new Map<string, OperationEntry>();
	private pending = new Map<string, Promise<OperationOutcome>>();
	private reviewed = new Set<string>();
	constructor(
		private changed: () => void = () => {},
		private storage?: Pick<Storage, "getItem" | "setItem">,
	) {
		try {
			const saved: unknown = JSON.parse(
				storage?.getItem("remote-agent-operation-ids") ?? "[]",
			);
			if (Array.isArray(saved))
				for (const entry of saved.slice(-512)) {
					if (
						typeof entry === "object" &&
						entry !== null &&
						"id" in entry &&
						"target" in entry &&
						typeof entry.id === "string" &&
						/^[A-Za-z0-9_-]{16,128}$/.test(entry.id) &&
						typeof entry.target === "string"
					) {
						this.entries.set(entry.id, {
							id: entry.id,
							target: entry.target,
							outcome: { status: "unknown", reason: "observationInterrupted" },
						});
					}
				}
		} catch {
			/* Storage availability never grants permission to replay. */
		}
	}
	private publish(): void {
		try {
			// Retain only IDs and namespaced target references: no credentials, prompts or file paths.
			this.storage?.setItem(
				"remote-agent-operation-ids",
				JSON.stringify(
					this.all()
						.filter(
							(entry) =>
								entry.outcome.status === "unknown" ||
								entry.outcome.status === "accepted",
						)
						.map(({ id, target }) => ({ id, target })),
				),
			);
		} catch {
			/* Rust retains its own at-most-once journal for this launch. */
		}
		this.changed();
	}
	all(): OperationEntry[] {
		return [...this.entries.values()];
	}
	/** Merge native restart records; restoring observations never sends a request. */
	restore(connection: ConnectionRef, records: JournalEntry[]): void {
		for (const record of records) {
			if (
				record.version !== 1 ||
				record.connection.providerId !== connection.providerId ||
				record.connection.connectionId !== connection.connectionId
			)
				throw new Error("Remote operation journal crossed its connection");
			const context = record.context;
			const previous = this.entries.get(record.operationId);
			const target = !context
				? (previous?.target ??
					JSON.stringify([
						connection.providerId,
						connection.connectionId,
						null,
					]))
				: context.type === "createSession"
					? workspaceKey(context.workspace)
					: sessionKey(context.session);
			if (!belongsToConnection(target, connection))
				throw new Error("Remote operation journal crossed its target");
			if (previous && previous.target !== target)
				throw new Error("Operation ID belongs to another target");
			if (this.pending.has(record.operationId)) continue;
			if (
				previous &&
				["succeeded", "rejected"].includes(previous.outcome.status)
			)
				continue;
			this.entries.set(record.operationId, {
				id: record.operationId,
				target,
				outcome: record.outcome,
			});
		}
		this.publish();
	}
	blocked(target: string): boolean {
		return this.all().some(
			(entry) =>
				entry.target === target &&
				entry.outcome.status === "unknown" &&
				!this.reviewed.has(entry.id),
		);
	}
	isReviewed(id: string): boolean {
		return this.reviewed.has(id);
	}
	allowNewActions(id: string): void {
		if (
			this.entries.get(id)?.outcome.status === "unknown" &&
			!this.pending.has(id)
		) {
			this.reviewed.add(id);
			this.publish();
		}
	}
	async dispatch(
		id: string,
		target: string,
		send: () => Promise<OperationOutcome>,
	): Promise<OperationOutcome> {
		const previous = this.entries.get(id);
		if (previous && previous.target !== target)
			throw new Error("Operation ID belongs to another target");
		const pending = this.pending.get(id);
		if (pending) return pending;
		if (previous) return previous.outcome;
		this.entries.set(id, {
			id,
			target,
			outcome: { status: "unknown", reason: "dispatchPending" },
		});
		this.publish();
		const work = (async (): Promise<OperationOutcome> => {
			let outcome: OperationOutcome;
			try {
				outcome = await send();
			} catch {
				outcome = { status: "unknown", reason: "ipcInterrupted" };
			}
			this.entries.set(id, { id, target, outcome });
			this.pending.delete(id);
			this.publish();
			return outcome;
		})();
		this.pending.set(id, work);
		return work;
	}
	inspect(id: string, receipt: ReceiptLookup): void {
		const entry = this.entries.get(id);
		// Missing/expired receipts never establish that a write did not execute.
		if (entry && receipt.status === "found") {
			this.entries.set(id, { ...entry, outcome: receipt.outcome });
			this.publish();
		}
	}
}
