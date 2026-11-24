import { open } from "@tauri-apps/plugin-dialog";
import {
	Check,
	ChevronDown,
	History,
	LoaderCircle,
	Monitor,
	Paperclip,
	Plus,
	RefreshCw,
	Send,
	ShieldCheck,
	Square,
} from "lucide-react";
import {
	type ReactNode,
	useCallback,
	useEffect,
	useId,
	useRef,
	useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
	Conversation,
	ConversationContent,
	ConversationEmptyState,
	ConversationScrollButton,
} from "@/components/agent/conversation";
import { Suggestion } from "@/components/agent/elements/suggestion";
import { MobileRemoteTranscript } from "@/components/mobile/remote/mobile-remote-transcript";
import { RemoteRegionContextPreview } from "@/components/mobile/remote/remote-region-context-preview";
import { DshRemoteSignIn } from "@/components/remote-agent/dsh-remote-sign-in";
import { RemoteModelMenu } from "@/components/remote-agent/remote-model-menu";
import { PaneHeader } from "@/components/shell/pane-header";
import { Button } from "@/components/ui/button";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuSeparator,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Textarea } from "@/components/ui/textarea";
import { suggestionItems } from "@/lib/agent/chat-state";
import type {
	ConnectionRef,
	HostInfo,
	HostRef,
	Mutation,
	OperationOutcome,
	Question,
	QuestionAnswer,
	RemoteAgentAccount,
	RemoteAgentCatalog,
	RemoteAgentHostIdentity,
	RemoteAttachment,
	SessionInfo,
	SessionRef,
	SessionSnapshot,
	WorkspaceInfo,
} from "@/lib/core/bindings";
import { errorText } from "@/lib/core/error";
import { notifyError } from "@/lib/core/notify";
import { cn } from "@/lib/core/utils";
import {
	type MobilePaperRegionContext,
	stagePaperRegionImage,
} from "@/lib/mobile/paper-context";
import {
	canRetrySavedSignIn,
	credentialMessageKey,
} from "@/lib/remote-agent/credentials";
import {
	belongsToConnection,
	canUpload,
	OperationJournal,
	sessionKey,
	supports,
	validAnswers,
	workspaceKey,
} from "@/lib/remote-agent/model";
import {
	RegionContextWorkflow,
	regionPromptValues,
	supportsRegionImage,
} from "@/lib/remote-agent/region-context-workflow";
import { remoteAgent } from "@/lib/remote-agent/service";

export type MobileRemoteAgentPageProps = {
	active?: boolean;
	mobile?: boolean;
	backendControl?: ReactNode;
	headerActions?: ReactNode;
	selection?: SessionRef | null;
	onSelectionChange?: (session: SessionRef | null) => void;
	regionContext?: MobilePaperRegionContext | null;
	onRegionContextHandled?: (id: string) => void;
};

/** Native remote sessions are shared by the mobile and desktop Agent surfaces. */
export function MobileRemoteAgentPage({
	active = true,
	mobile = true,
	backendControl,
	headerActions,
	selection,
	onSelectionChange,
	regionContext,
	onRegionContextHandled,
}: MobileRemoteAgentPageProps) {
	const { t } = useTranslation("mobile");
	const { t: agentText } = useTranslation("agent");
	const draftRef = useRef<HTMLTextAreaElement>(null);
	const [catalog, setCatalog] = useState<RemoteAgentCatalog | null>(null);
	const [account, setAccount] = useState<RemoteAgentAccount | null>(null);
	const [hosts, setHosts] = useState<HostInfo[]>([]);
	const [workspaces, setWorkspaces] = useState<WorkspaceInfo[]>([]);
	const [selectedHost, setSelectedHost] = useState<HostRef | null>(
		selection?.workspace.host ?? null,
	);
	const [connectedHost, setConnectedHost] = useState<HostRef | null>(null);
	// The native catalog preserves the Host's workspace order.
	const defaultWorkspace = workspaces[0]?.workspace ?? null;
	const [sessions, setSessions] = useState<SessionInfo[]>([]);
	const [session, setSession] = useState<SessionRef | null>(selection ?? null);
	const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
	const [identity, setIdentity] = useState<RemoteAgentHostIdentity | null>(
		null,
	);
	const [pendingRegion, setPendingRegion] =
		useState<MobilePaperRegionContext | null>(null);
	const [includeRegionImage, setIncludeRegionImage] = useState(true);
	const handledRegionIds = useRef(new Set<string>());
	const currentRegionId = useRef<string | null>(null);
	currentRegionId.current = pendingRegion?.id ?? null;
	const mounted = useRef(true);
	const regionWorkflowRef = useRef<RegionContextWorkflow | null>(null);
	if (!regionWorkflowRef.current)
		regionWorkflowRef.current = new RegionContextWorkflow({
			stage: stagePaperRegionImage,
			upload: remoteAgent.upload,
			send: remoteAgent.execute,
		});
	const regionWorkflow = regionWorkflowRef.current;
	const [draft, setDraft] = useState("");
	const [attachments, setAttachments] = useState<RemoteAttachment[]>([]);
	const [busy, setBusy] = useState(false);
	const [loading, setLoading] = useState(false);
	const [revision, setRevision] = useState(0);
	const [journalConnection, setJournalConnection] = useState<string | null>(
		null,
	);
	const [journalFault, setJournalFault] = useState(false);
	const journalEpoch = useRef(0);
	const busyRef = useRef(false);
	const journalRef = useRef<OperationJournal | null>(null);
	if (!journalRef.current) {
		let storage: Storage | undefined;
		try {
			storage = window.sessionStorage;
		} catch {
			/* Storage may be unavailable. */
		}
		journalRef.current = new OperationJournal(
			() => setRevision((value) => value + 1),
			storage,
		);
	}
	const journal = journalRef.current;
	const capabilities = account?.info.capabilities;
	const credentialMessage = catalog
		? credentialMessageKey(catalog.credentialState)
		: null;
	const selectedKey = session ? sessionKey(session) : null;
	const currentSelectionKey = useRef(selectedKey);
	currentSelectionKey.current = selectedKey;
	const connectionKey = account
		? JSON.stringify([
				account.info.connection.providerId,
				account.info.connection.connectionId,
			])
		: null;
	const currentConnectionKey = useRef(connectionKey);
	currentConnectionKey.current = connectionKey;
	const regionState = pendingRegion
		? regionWorkflow.state(pendingRegion.id, selectedKey, journal)
		: null;
	const regionImageSupported =
		supportsRegionImage(capabilities) || Boolean(regionState?.uploaded);
	const regionBlocked = Boolean(
		regionState?.uncertain ||
			(pendingRegion && includeRegionImage && !regionImageSupported),
	);
	const journalReady = Boolean(
		connectionKey && journalConnection === connectionKey,
	);
	const blocked =
		!journalReady || (selectedKey ? journal.blocked(selectedKey) : false);
	const workspaceTarget = defaultWorkspace
		? workspaceKey(defaultWorkspace)
		: null;
	const unknownOperations = account
		? journal
				.all()
				.filter(
					(entry) =>
						belongsToConnection(entry.target, account.info.connection) &&
						(entry.target === selectedKey ||
							entry.target === workspaceTarget) &&
						entry.outcome.status === "unknown" &&
						entry.outcome.reason !== "dispatchPending" &&
						!journal.isReviewed(entry.id),
				)
		: [];
	const canCreate = Boolean(
		workspaceTarget &&
			journalReady &&
			!journal.blocked(workspaceTarget) &&
			supports(capabilities, "createSession"),
	);
	const composerDisabled =
		busy ||
		blocked ||
		regionBlocked ||
		!supports(capabilities, "sendMessage") ||
		(!session && !canCreate);
	void revision;

	function chooseSession(value: SessionRef | null) {
		currentSelectionKey.current = value ? sessionKey(value) : null;
		setSession(value);
		setSnapshot(null);
		setDraft("");
		setAttachments([]);
		onSelectionChange?.(value);
	}
	const adoptCatalog = useCallback(
		(value: RemoteAgentCatalog) => {
			const nextConnection = value.account
				? value.account.info.connection
				: null;
			const nextKey = nextConnection
				? JSON.stringify([
						nextConnection.providerId,
						nextConnection.connectionId,
					])
				: null;
			const changed =
				currentConnectionKey.current !== null &&
				currentConnectionKey.current !== nextKey;
			currentConnectionKey.current = nextKey;
			setCatalog(value);
			// Keep polling dependencies stable when a failed read only changes credential status.
			setAccount((previous) =>
				JSON.stringify(previous) === JSON.stringify(value.account)
					? previous
					: value.account,
			);
			if (changed || !nextConnection) {
				currentSelectionKey.current = null;
				setSession(null);
				setSnapshot(null);
				setDraft("");
				setAttachments([]);
				setHosts([]);
				setWorkspaces([]);
				setSelectedHost(null);
				setConnectedHost(null);
				setIdentity(null);
				setSessions([]);
				onSelectionChange?.(null);
			}
		},
		[onSelectionChange],
	);
	async function reloadCatalog() {
		const value = await remoteAgent.catalog();
		adoptCatalog(value);
		return value;
	}
	async function clearSignIn() {
		await remoteAgent.logout();
		await reloadCatalog();
	}
	function finishRegion(id: string, clearComposer: boolean) {
		if (currentRegionId.current !== id) return;
		currentRegionId.current = null;
		handledRegionIds.current.add(id);
		while (handledRegionIds.current.size > 128) {
			const oldest = handledRegionIds.current.values().next().value;
			if (oldest === undefined) break;
			handledRegionIds.current.delete(oldest);
		}
		regionWorkflow.discard(id);
		setPendingRegion((current) => (current?.id === id ? null : current));
		onRegionContextHandled?.(id);
		if (clearComposer) {
			setDraft("");
			setAttachments([]);
		}
	}
	async function inspectOperation(id: string, connection: ConnectionRef) {
		const expected = JSON.stringify([
			connection.providerId,
			connection.connectionId,
		]);
		if (currentConnectionKey.current !== expected) {
			notifyError(t("paperContext.errors.staleTarget"));
			return;
		}
		const receipt = await remoteAgent.receipt(connection, id);
		journal.inspect(id, receipt);
		const consumed = regionWorkflow.observedSend(id, journal);
		if (
			mounted.current &&
			currentConnectionKey.current === expected &&
			consumed?.target === currentSelectionKey.current
		)
			finishRegion(consumed.contextId, true);
		if (mounted.current && currentConnectionKey.current === expected)
			await refresh();
	}
	useEffect(() => {
		mounted.current = true;
		return () => {
			mounted.current = false;
		};
	}, []);
	useEffect(() => {
		if (
			!pendingRegion &&
			regionContext &&
			!handledRegionIds.current.has(regionContext.id)
		) {
			setPendingRegion(regionContext);
			setIncludeRegionImage(true);
		}
	}, [regionContext, pendingRegion]);
	async function run<T>(action: () => Promise<T>): Promise<T | undefined> {
		if (busyRef.current) return;
		busyRef.current = true;
		setBusy(true);
		try {
			return await action();
		} catch (error) {
			notifyError(errorText(error));
			try {
				await reloadCatalog();
			} catch {
				/* Keep the last observed account if IPC itself is unavailable. */
			}
		} finally {
			busyRef.current = false;
			setBusy(false);
		}
	}
	useEffect(() => {
		if (selection === undefined) return;
		const nextKey = selection ? sessionKey(selection) : null;
		if (nextKey !== currentSelectionKey.current) {
			setSnapshot(null);
			setDraft("");
			setAttachments([]);
		}
		currentSelectionKey.current = nextKey;
		setSession(selection);
	}, [selection]);
	useEffect(() => {
		if (!active) return;
		let cancelled = false;
		void remoteAgent
			.catalog()
			.then(async (value) => {
				if (cancelled) return;
				adoptCatalog(value);
				if (value.account) {
					const rows = await remoteAgent.hosts(value.account.info.connection);
					if (!cancelled) setHosts(rows);
				}
			})
			.catch((error) => {
				if (!cancelled) notifyError(errorText(error));
			});
		return () => {
			cancelled = true;
		};
	}, [active, adoptCatalog]);
	const restoreOperations = useCallback(async () => {
		if (!account) return;
		const epoch = ++journalEpoch.current;
		setJournalConnection(null);
		setJournalFault(false);
		const connection = account.info.connection;
		try {
			const records = await remoteAgent.localOperations(connection);
			if (epoch !== journalEpoch.current) return;
			journal.restore(connection, records);
			setJournalConnection(
				JSON.stringify([connection.providerId, connection.connectionId]),
			);
		} catch {
			if (epoch === journalEpoch.current) {
				setJournalFault(true);
				notifyError(t("remote.journalUnavailable"));
			}
		}
	}, [account, journal, t]);
	useEffect(() => {
		if (!active) return;
		void restoreOperations();
		return () => {
			journalEpoch.current++;
		};
	}, [active, restoreOperations]);
	// Official snapshots include history and currently-owned interactions. Never replay writes on focus/reconnect.
	useEffect(() => {
		if (
			!active ||
			!session ||
			!account ||
			!belongsToConnection(sessionKey(session), account.info.connection) ||
			!supports(capabilities, "readSession")
		)
			return;
		let cancelled = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		setLoading(true);
		const read = async () => {
			try {
				const value = await remoteAgent.read(session);
				if (!cancelled) {
					setSnapshot(value);
					setLoading(false);
					timer = setTimeout(read, 3000);
				}
			} catch (error) {
				if (!cancelled) {
					setLoading(false);
					notifyError(errorText(error));
					try {
						const value = await remoteAgent.catalog();
						if (!cancelled) adoptCatalog(value);
					} catch {
						/* Keep the observed history. */
					}
				}
			}
		};
		void read();
		return () => {
			cancelled = true;
			clearTimeout(timer);
		};
	}, [active, session, account, capabilities, adoptCatalog]);

	async function refresh() {
		if (session && supports(capabilities, "readSession")) {
			const value = await remoteAgent.read(session);
			if (currentSelectionKey.current === sessionKey(session))
				setSnapshot(value);
		}
	}
	async function loadSessionHistory() {
		const history: SessionInfo[] = [];
		for (const { workspace } of workspaces) {
			history.push(...(await remoteAgent.sessions(workspace)));
		}
		setSessions(history);
	}
	async function mutate(
		mutation: Mutation,
		target: string,
	): Promise<OperationOutcome> {
		if (!journalReady) throw new Error(t("remote.journalUnavailable"));
		const operationId = crypto.randomUUID();
		const outcome = await journal.dispatch(operationId, target, () =>
			remoteAgent.execute({ operationId, mutation }),
		);
		if (outcome.status === "rejected") {
			notifyError(outcome.message);
			await reloadCatalog();
		}
		return outcome;
	}
	async function signedIn(value: RemoteAgentAccount) {
		if (catalog)
			adoptCatalog({
				...catalog,
				account: value,
				credentialState: "ready",
				persistence: value.persistence,
				canClearSavedSignIn: value.persistence === "systemStore",
			});
		else setAccount(value);
		setHosts(await remoteAgent.hosts(value.info.connection));
	}
	async function connectHost(value: RemoteAgentHostIdentity) {
		if (!account) return;
		const info = await remoteAgent.trust(value.host, value.fingerprint);
		const rows = supports(info.capabilities, "listWorkspaces")
			? await remoteAgent.workspaces(value.host)
			: [];
		setAccount({ ...account, info });
		setIdentity(null);
		setConnectedHost(value.host);
		setHosts((current) =>
			current.map((host) =>
				host.host.hostId === value.host.hostId
					? { ...host, pinned: true }
					: host,
			),
		);
		setWorkspaces(rows);
	}
	async function selectHost(host: HostInfo) {
		if (selectedHost?.hostId === host.host.hostId && connectedHost) return;
		const value = await remoteAgent.identity(host.host);
		setSelectedHost(value.host);
		setConnectedHost(null);
		setWorkspaces([]);
		setSessions([]);
		chooseSession(null);
		setIdentity(value);
		if (value.pinned) await connectHost(value);
	}
	async function createSession() {
		if (!defaultWorkspace || !workspaceTarget || !canCreate) return null;
		const originalConnection = currentConnectionKey.current;
		const originalSelection = currentSelectionKey.current;
		const outcome = await mutate(
			{ type: "createSession", workspace: defaultWorkspace, title: null },
			workspaceTarget,
		);
		if (
			outcome.status === "succeeded" &&
			outcome.result.type === "sessionCreated" &&
			mounted.current &&
			currentConnectionKey.current === originalConnection &&
			currentSelectionKey.current === originalSelection
		) {
			const next = outcome.result.session;
			currentSelectionKey.current = sessionKey(next);
			setSession(next);
			setSnapshot(null);
			onSelectionChange?.(next);
			return next;
		}
		return null;
	}
	async function send() {
		if (
			blocked ||
			regionBlocked ||
			(!draft.trim() && !attachments.length && !pendingRegion)
		)
			return;
		const capturedSession = session ?? (await createSession());
		if (!capturedSession) return;
		const target = sessionKey(capturedSession);
		const capturedConnection = connectionKey;
		const current = () =>
			mounted.current &&
			currentSelectionKey.current === target &&
			currentConnectionKey.current === capturedConnection;
		if (pendingRegion) {
			const context = pendingRegion;
			const result = await regionWorkflow.submit({
				context,
				includeImage: includeRegionImage,
				session: capturedSession,
				capabilities,
				text: draft,
				attachments,
				journal,
				isCurrent: () => current() && currentRegionId.current === context.id,
				formatReference: (source, imageAttached) =>
					t("paperContext.prompt", {
						...regionPromptValues(source),
						imageStatus: t(
							imageAttached
								? "paperContext.imageAttached"
								: "paperContext.imageOmitted",
						),
					}),
				onCleanupFailure: () => notifyError(t("paperContext.errors.cleanup")),
			});
			if (result.kind === "sent" && result.current)
				finishRegion(result.contextId, true);
			else if (result.kind === "alreadySent" && result.current) {
				finishRegion(result.contextId, false);
				notifyError(t("paperContext.errors.alreadySent"));
			} else if (
				result.kind === "rejected" &&
				result.outcome?.status === "rejected"
			) {
				notifyError(result.outcome.message);
				if (current()) await reloadCatalog();
			} else if (result.kind === "invalidImage")
				notifyError(t("paperContext.errors.invalidImage"));
			else if (result.kind === "attachmentMismatch")
				notifyError(t("paperContext.errors.attachmentMismatch"));
			else if (
				result.kind === "stale" ||
				(!result.current && result.kind === "sent")
			)
				notifyError(t("paperContext.errors.staleTarget"));
			else if (result.kind === "unsupportedImage")
				notifyError(t("paperContext.referenceOnly"));
			if (current()) await refresh();
			return;
		}
		const outcome = await mutate(
			{
				type: "sendMessage",
				session: capturedSession,
				text: draft,
				attachments,
			},
			target,
		);
		if (
			current() &&
			(outcome.status === "accepted" || outcome.status === "succeeded")
		) {
			setDraft("");
			setAttachments([]);
		}
		if (current()) await refresh();
	}

	async function upload() {
		if (!journalReady || !session || !canUpload(capabilities) || blocked)
			return;
		const path = await open({
			multiple: false,
			directory: false,
			title: t("remote.uploadPrivate"),
		});
		if (typeof path !== "string") return;
		const id = crypto.randomUUID();
		const outcome = await journal.dispatch(id, sessionKey(session), () =>
			remoteAgent.upload(session, path, id),
		);
		if (
			outcome.status === "succeeded" &&
			outcome.result.type === "attachmentUploaded"
		) {
			const attachment = outcome.result.attachment;
			setAttachments((current) => [...current, attachment]);
		}
		if (outcome.status === "rejected") {
			notifyError(outcome.message);
			await reloadCatalog();
		}
	}

	const actionClass = mobile ? "min-h-11 min-w-11" : "size-8";
	const selectedHostName = hosts.find(
		(host) => host.host.hostId === selectedHost?.hostId,
	)?.name;
	return (
		<section
			className="flex h-full min-h-0 flex-col bg-sidebar"
			aria-label={t("remote.title")}
		>
			<PaneHeader
				className={mobile ? "h-12 gap-0.5 px-1" : undefined}
				trailing={
					<>
						{connectedHost && (
							<>
								<Button
									type="button"
									variant="ghost"
									size="icon-sm"
									className={actionClass}
									aria-label={t("remote.newSession")}
									title={t("remote.newSession")}
									disabled={busy || !canCreate}
									onClick={() =>
										void run(async () => {
											if (await createSession()) {
												setDraft("");
												setAttachments([]);
											}
										})
									}
								>
									<Plus className="size-4" />
								</Button>
								<DropdownMenu
									onOpenChange={(open) => {
										if (open && supports(capabilities, "listSessions"))
											void run(loadSessionHistory);
									}}
								>
									<DropdownMenuTrigger asChild>
										<Button
											type="button"
											variant="ghost"
											size="icon-sm"
											className={actionClass}
											aria-label={t("agent.history")}
											title={t("agent.history")}
										>
											<History className="size-4" />
										</Button>
									</DropdownMenuTrigger>
									<DropdownMenuContent
										align="end"
										className="max-h-72 w-64 overflow-y-auto"
									>
										{sessions.length ? (
											sessions.map((value) => (
												<DropdownMenuItem
													key={sessionKey(value.session)}
													className={cn("gap-2", mobile && "min-h-11")}
													disabled={
														busy || !supports(capabilities, "readSession")
													}
													onSelect={() => chooseSession(value.session)}
												>
													<span className="min-w-0 flex-1 truncate">
														{value.title || t("remote.untitled")}
													</span>
													{sessionKey(value.session) === selectedKey && (
														<Check className="size-4 shrink-0" />
													)}
												</DropdownMenuItem>
											))
										) : (
											<p className="px-2 py-1.5 text-muted-foreground text-sm">
												{t(busy ? "remote.loading" : "remote.noSessions")}
											</p>
										)}
									</DropdownMenuContent>
								</DropdownMenu>
							</>
						)}
						{headerActions}
					</>
				}
			>
				{backendControl ?? <span className="font-medium">dsh-remote</span>}
				{account && (
					<DropdownMenu>
						<DropdownMenuTrigger asChild disabled={busy}>
							<Button
								type="button"
								variant="ghost"
								size="sm"
								title={selectedHostName ?? t("remote.hosts")}
								aria-label={t("remote.hosts")}
								className={cn(
									"min-w-0 shrink gap-1 px-1.5",
									mobile ? "h-11" : "h-7",
								)}
							>
								<Monitor className="size-4 shrink-0" />
								<span className="truncate">
									{selectedHostName ?? t("remote.hosts")}
								</span>
								<ChevronDown className="size-3 shrink-0 opacity-70" />
							</Button>
						</DropdownMenuTrigger>
						<DropdownMenuContent align="start" className="w-60">
							{hosts.map((host) => (
								<DropdownMenuItem
									key={host.host.hostId}
									className={cn("gap-2", mobile && "min-h-11")}
									disabled={host.online === false}
									onSelect={() => void run(() => selectHost(host))}
								>
									<span className="min-w-0 flex-1 truncate">{host.name}</span>
									{host.pinned && (
										<ShieldCheck
											className="size-4 shrink-0 text-emerald-600"
											aria-label={t("remote.trusted")}
										/>
									)}
									<span className="text-muted-foreground text-xs">
										{t(
											host.online === true
												? "remote.online"
												: host.online === false
													? "remote.offline"
													: "remote.presenceUnknown",
										)}
									</span>
									{host.host.hostId === selectedHost?.hostId && (
										<Check className="size-4 shrink-0" />
									)}
								</DropdownMenuItem>
							))}
							<DropdownMenuSeparator />
							<DropdownMenuItem
								className={mobile ? "min-h-11" : undefined}
								onSelect={() =>
									void run(async () => {
										setHosts(await remoteAgent.hosts(account.info.connection));
										if (
											connectedHost &&
											supports(capabilities, "listWorkspaces")
										)
											setWorkspaces(
												await remoteAgent.workspaces(connectedHost),
											);
										await refresh();
									})
								}
							>
								<RefreshCw className="size-4" />
								{t("remote.refreshHosts")}
							</DropdownMenuItem>
						</DropdownMenuContent>
					</DropdownMenu>
				)}
			</PaneHeader>
			<Conversation key={selectedKey ?? "draft"} className="min-h-0">
				<ConversationContent className="gap-3 p-3">
					{catalog && credentialMessage && account && (
						<div className="m-4 space-y-3 rounded-lg border p-3" role="status">
							<p className="text-sm">{t(credentialMessage)}</p>
							<div className="flex flex-wrap gap-2">
								{canRetrySavedSignIn(catalog.credentialState) && (
									<Button
										variant="outline"
										className="min-h-12"
										disabled={busy}
										onClick={() =>
											void run(async () => {
												const value = await reloadCatalog();
												if (value.account)
													setHosts(
														await remoteAgent.hosts(
															value.account.info.connection,
														),
													);
											})
										}
									>
										{t("remote.credentials.retry")}
									</Button>
								)}
								{catalog.canClearSavedSignIn && (
									<Button
										variant="outline"
										className="min-h-12"
										disabled={busy}
										onClick={() => void run(clearSignIn)}
									>
										{t("remote.credentials.clear")}
									</Button>
								)}
							</div>
						</div>
					)}
					{!account ? (
						<div className="mx-auto flex w-full max-w-md flex-col gap-4 p-2">
							{catalog ? (
								<DshRemoteSignIn
									mobile={mobile}
									onCatalog={adoptCatalog}
									onSignedIn={(value) => void run(() => signedIn(value))}
								/>
							) : (
								<p className="text-muted-foreground text-sm">
									{t("remote.loading")}
								</p>
							)}
						</div>
					) : identity ? (
						<div className="flex flex-col items-center gap-3 p-3 text-center">
							<p className="break-words font-medium">{identity.name}</p>
							<Button
								disabled={busy}
								className={mobile ? "min-h-11" : undefined}
								onClick={() => void run(() => connectHost(identity))}
							>
								<ShieldCheck className="size-4" />
								{t("remote.trustAndConnect")}
							</Button>
						</div>
					) : !connectedHost && !session ? (
						<p className="px-3 py-5 text-center text-muted-foreground text-sm">
							{t(
								busy
									? "remote.loading"
									: hosts.length
										? "remote.chooseHost"
										: "remote.noHosts",
							)}
						</p>
					) : loading && !snapshot ? (
						<LoaderCircle
							className="mx-auto size-4 animate-spin motion-reduce:animate-none"
							aria-label={t("remote.loading")}
						/>
					) : !session && !defaultWorkspace ? (
						<p className="px-3 py-5 text-center text-muted-foreground text-sm">
							{t("remote.noWorkspace")}
						</p>
					) : !session ||
						(snapshot?.events.length === 0 &&
							snapshot.info.status === "idle") ? (
						<ConversationEmptyState className="p-3">
							<div className="flex w-full max-w-sm flex-col gap-2">
								{suggestionItems(false).map(({ key }) => (
									<Suggestion
										key={key}
										suggestion={agentText(`suggestions.${key}`)}
										className={cn(
											"h-auto justify-start whitespace-normal rounded-lg px-3 py-2.5 text-left",
											mobile && "min-h-11",
										)}
										disabled={composerDisabled}
										onClick={(value) => {
											setDraft(value);
											draftRef.current?.focus();
										}}
									/>
								))}
							</div>
						</ConversationEmptyState>
					) : null}
					{snapshot && session && (
						<MobileRemoteTranscript
							events={snapshot.events}
							sessionRunning={snapshot.info.status === "running"}
							renderInteraction={(event) => (
								<>
									{event.payload.type === "permissionRequest" && (
										<div className="space-y-3 rounded-lg border p-3">
											<p className="font-medium">{t("remote.permission")}</p>
											<p className="whitespace-pre-wrap break-words text-sm">
												{event.payload.description}
											</p>
											<div className="flex flex-wrap gap-2">
												{event.payload.options.map((option) => (
													<Button
														className="min-h-12"
														variant="outline"
														key={option}
														disabled={
															busy ||
															blocked ||
															!supports(capabilities, "respondPermission")
														}
														onClick={() =>
															void run(async () => {
																if (event.payload.type !== "permissionRequest")
																	return;
																await mutate(
																	{
																		type: "respondPermission",
																		session,
																		requestId: event.payload.requestId,
																		optionId: option,
																	},
																	sessionKey(session),
																);
																await refresh();
															})
														}
													>
														{option === "allowed-once"
															? t("remote.allowOnce")
															: option === "rejected"
																? t("remote.reject")
																: option}
													</Button>
												))}
											</div>
										</div>
									)}
									{event.payload.type === "questionRequest" && (
										<QuestionForm
											questions={event.payload.questions}
											disabled={
												busy ||
												blocked ||
												!supports(capabilities, "respondQuestion")
											}
											onSubmit={(answers) =>
												void run(async () => {
													if (event.payload.type !== "questionRequest") return;
													await mutate(
														{
															type: "respondQuestion",
															session,
															requestId: event.payload.requestId,
															answers,
														},
														sessionKey(session),
													);
													await refresh();
												})
											}
										/>
									)}
								</>
							)}
						/>
					)}
					{account && journalFault && (
						<div className="flex items-center gap-3 p-2 text-sm" role="status">
							<p className="flex-1 text-muted-foreground">
								{t("remote.journalUnavailable")}
							</p>
							<Button
								variant="outline"
								onClick={() => void restoreOperations()}
							>
								{t("local.retry")}
							</Button>
						</div>
					)}
					{account &&
						unknownOperations.map((entry) => (
							<div
								key={entry.id}
								className="space-y-2 rounded-md bg-muted/40 p-3 text-sm"
								role="status"
							>
								<p>{t("remote.unknownOutcome")}</p>
								<div className="flex flex-wrap gap-2">
									{supports(capabilities, "operationReceipts") && (
										<Button
											variant="outline"
											disabled={busy}
											onClick={() =>
												void run(() =>
													inspectOperation(entry.id, account.info.connection),
												)
											}
										>
											{t("remote.inspectReceipt")}
										</Button>
									)}
									<Button
										variant="outline"
										disabled={busy}
										onClick={() =>
											void run(async () => {
												await refresh();
												if (
													entry.target === selectedKey &&
													!regionWorkflow.ownsOperation(entry.id)
												) {
													setDraft("");
													setAttachments([]);
												}
												journal.allowNewActions(entry.id);
											})
										}
									>
										{t("remote.continueNewTask")}
									</Button>
								</div>
							</div>
						))}
				</ConversationContent>
				<ConversationScrollButton />
			</Conversation>
			{account && (session || defaultWorkspace || pendingRegion) && (
				<form
					className="m-2 shrink-0 space-y-1.5 rounded-xl border bg-background p-2"
					onSubmit={(event) => {
						event.preventDefault();
						void run(send);
					}}
				>
					{pendingRegion && (
						<RemoteRegionContextPreview
							context={pendingRegion}
							includeImage={includeRegionImage}
							imageSupported={capabilities ? regionImageSupported : true}
							uncertain={Boolean(regionState?.uncertain)}
							uploaded={Boolean(regionState?.uploaded)}
							busy={busy}
							onIncludeImage={setIncludeRegionImage}
							onRemove={() => finishRegion(pendingRegion.id, false)}
							onInspect={
								supports(capabilities, "operationReceipts") &&
								regionWorkflow.pendingOperations(pendingRegion.id, journal)
									.length
									? () =>
											void run(async () => {
												for (const operation of regionWorkflow.pendingOperations(
													pendingRegion.id,
													journal,
												))
													await inspectOperation(
														operation.id,
														operation.session.workspace.host.connection,
													);
											})
									: undefined
							}
						/>
					)}
					{attachments.map((attachment) => (
						<p key={attachment.path} className="break-all text-sm">
							{attachment.path}
						</p>
					))}
					<Textarea
						ref={draftRef}
						rows={2}
						value={draft}
						onChange={(event) => setDraft(event.target.value)}
						placeholder={t("remote.message")}
						aria-label={t("remote.message")}
						className={cn(
							"max-h-36 min-h-12 resize-none border-transparent bg-transparent shadow-none",
							mobile && "text-base md:text-base",
						)}
						disabled={composerDisabled}
					/>
					<div className="flex items-center justify-end gap-1">
						{session && supports(capabilities, "selectModel") && (
							<RemoteModelMenu
								key={selectedKey}
								session={session}
								mobile={mobile}
								disabled={
									busy || blocked || snapshot?.info.status === "running"
								}
								onSelect={async (selection) =>
									(await run(async () => {
										const outcome = await mutate(
											{ type: "selectModel", session, selection },
											sessionKey(session),
										);
										return outcome.status === "succeeded" &&
											outcome.result.type === "modelSelected"
											? outcome.result.selected
											: null;
									})) ?? null
								}
							/>
						)}
						{session && canUpload(capabilities) && (
							<Button
								type="button"
								variant="ghost"
								size="icon-sm"
								className={actionClass}
								disabled={busy || blocked || Boolean(regionState?.uncertain)}
								aria-label={t("remote.uploadPrivate")}
								title={t("remote.uploadPrivate")}
								onClick={() => void run(upload)}
							>
								<Paperclip className="size-4" />
							</Button>
						)}
						{session &&
							(snapshot?.info.status === "running" ||
								snapshot?.info.status === "waitingForPermission" ||
								snapshot?.info.status === "waitingForAnswer") &&
							supports(capabilities, "cancel") && (
								<Button
									type="button"
									variant="ghost"
									size="icon-sm"
									className={actionClass}
									disabled={busy || blocked}
									aria-label={t("remote.cancel")}
									onClick={() =>
										void run(async () => {
											await mutate(
												{ type: "cancel", session },
												sessionKey(session),
											);
											await refresh();
										})
									}
								>
									<Square className="size-4" />
								</Button>
							)}
						<Button
							type="submit"
							size="icon-sm"
							className={actionClass}
							aria-label={t("remote.send")}
							disabled={
								composerDisabled ||
								(!draft.trim() && !attachments.length && !pendingRegion)
							}
						>
							<Send className="size-4" />
						</Button>
					</div>
				</form>
			)}
		</section>
	);
}

function QuestionForm({
	questions,
	disabled,
	onSubmit,
}: {
	questions: Question[];
	disabled: boolean;
	onSubmit: (answers: QuestionAnswer[]) => void;
}) {
	const { t } = useTranslation("mobile");
	const groupId = useId();
	const [answers, setAnswers] = useState<QuestionAnswer[]>(() =>
		questions.map((question) => ({
			questionId: question.id,
			selected: [],
			custom: null,
		})),
	);
	const [skipped, setSkipped] = useState<string[]>([]);
	function update(index: number, value: Partial<QuestionAnswer>) {
		setSkipped((current) => current.filter((id) => id !== questions[index].id));
		setAnswers((current) =>
			current.map((answer, position) =>
				position === index ? { ...answer, ...value } : answer,
			),
		);
	}
	return (
		<form
			className="space-y-4 rounded-lg border p-3"
			onSubmit={(event) => {
				event.preventDefault();
				if (validAnswers(questions, answers, skipped)) onSubmit(answers);
			}}
		>
			{questions.map((question, index) => (
				<fieldset className="space-y-2" key={question.id} disabled={disabled}>
					<legend className="mb-2 font-medium">{question.prompt}</legend>
					<Button
						type="button"
						className="min-h-11"
						variant={skipped.includes(question.id) ? "secondary" : "outline"}
						onClick={() => {
							update(index, { selected: [], custom: null });
							setSkipped((current) => [...current, question.id]);
						}}
					>
						{t("remote.skipQuestion")}
					</Button>
					{question.options.map((option) => (
						<label
							key={option}
							className="flex min-h-12 items-center gap-3 rounded-md border px-3"
						>
							<input
								type={question.multiple ? "checkbox" : "radio"}
								name={`${groupId}-${question.id}`}
								checked={answers[index]?.selected.includes(option) ?? false}
								onChange={(event) =>
									update(index, {
										selected: question.multiple
											? event.target.checked
												? [...answers[index].selected, option]
												: answers[index].selected.filter(
														(value) => value !== option,
													)
											: [option],
									})
								}
							/>
							<span>{option}</span>
						</label>
					))}
					{question.allowsCustom && (
						<Textarea
							className="min-h-12 text-base"
							aria-label={t("remote.customAnswer")}
							placeholder={t("remote.customAnswer")}
							value={answers[index]?.custom ?? ""}
							onChange={(event) =>
								update(index, { custom: event.target.value || null })
							}
						/>
					)}
				</fieldset>
			))}
			<Button
				type="submit"
				className="min-h-12"
				disabled={disabled || !validAnswers(questions, answers, skipped)}
			>
				{t("remote.answer")}
			</Button>
		</form>
	);
}
