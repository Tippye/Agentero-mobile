import { open } from "@tauri-apps/plugin-dialog";
import {
	LoaderCircle,
	Paperclip,
	Plus,
	RefreshCw,
	Send,
	Square,
} from "lucide-react";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
	Message,
	MessageContent,
	MessageResponse,
} from "@/components/agent/message";
import { RemoteRegionContextPreview } from "@/components/mobile/remote/remote-region-context-preview";
import { DshRemoteSignIn } from "@/components/remote-agent/dsh-remote-sign-in";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import type {
	ConnectionRef,
	HostInfo,
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
	validTrustConfirmation,
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
	selection?: SessionRef | null;
	onSelectionChange?: (session: SessionRef | null) => void;
	regionContext?: MobilePaperRegionContext | null;
	onRegionContextHandled?: (id: string) => void;
};

/** DSH is the default provider; the existing Bridge page remains a separate parent choice. */
export function MobileRemoteAgentPage({
	active = true,
	selection,
	onSelectionChange,
	regionContext,
	onRegionContextHandled,
}: MobileRemoteAgentPageProps) {
	const { t } = useTranslation("mobile");
	const formId = useId();
	const [catalog, setCatalog] = useState<RemoteAgentCatalog | null>(null);
	const [account, setAccount] = useState<RemoteAgentAccount | null>(null);
	const [providerId, setProviderId] = useState("");
	const [hosts, setHosts] = useState<HostInfo[]>([]);
	const [workspaces, setWorkspaces] = useState<WorkspaceInfo[]>([]);
	const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
	const [sessions, setSessions] = useState<SessionInfo[]>([]);
	const [session, setSession] = useState<SessionRef | null>(selection ?? null);
	const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
	const [identity, setIdentity] = useState<RemoteAgentHostIdentity | null>(
		null,
	);
	const [confirmation, setConfirmation] = useState("");
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
	const entries = account
		? journal
				.all()
				.filter((entry) =>
					belongsToConnection(entry.target, account.info.connection),
				)
		: [];
	void revision;

	function chooseSession(value: SessionRef | null) {
		currentSelectionKey.current = value ? sessionKey(value) : null;
		setSession(value);
		setSnapshot(null);
		setDraft("");
		setAttachments([]);
		onSelectionChange?.(value);
	}
	const adoptCatalog = useCallback((value: RemoteAgentCatalog) => {
		currentConnectionKey.current = value.account
			? JSON.stringify([
					value.account.info.connection.providerId,
					value.account.info.connection.connectionId,
				])
			: null;
		setCatalog(value);
		setProviderId(
			value.account?.info.connection.providerId ?? value.defaultProvider,
		);
		// Keep polling dependencies stable when a failed read only changes credential status.
		setAccount((previous) =>
			JSON.stringify(previous) === JSON.stringify(value.account)
				? previous
				: value.account,
		);
		if (!value.account) {
			currentSelectionKey.current = null;
			setSession(null);
			setSnapshot(null);
			setAttachments([]);
		}
	}, []);
	async function reloadCatalog() {
		const value = await remoteAgent.catalog();
		adoptCatalog(value);
		return value;
	}
	async function clearSignIn() {
		await remoteAgent.logout();
		await reloadCatalog();
		setHosts([]);
		setWorkspaces([]);
		setWorkspace(null);
		setSessions([]);
		chooseSession(null);
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
	async function run(action: () => Promise<void>) {
		if (busyRef.current) return;
		busyRef.current = true;
		setBusy(true);
		try {
			await action();
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
		if (!active || !session || !supports(capabilities, "readSession")) return;
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
	}, [active, session, capabilities, adoptCatalog]);

	async function refresh() {
		if (session && supports(capabilities, "readSession")) {
			const value = await remoteAgent.read(session);
			if (currentSelectionKey.current === sessionKey(session))
				setSnapshot(value);
		}
		if (workspace && supports(capabilities, "listSessions"))
			setSessions(await remoteAgent.sessions(workspace.workspace));
		if (account && supports(capabilities, "listHosts"))
			setHosts(await remoteAgent.hosts(account.info.connection));
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
		setAccount(value);
		await reloadCatalog();
		setHosts(await remoteAgent.hosts(value.info.connection));
	}
	async function trustHost() {
		if (
			!identity ||
			!account ||
			!validTrustConfirmation(identity, confirmation)
		)
			return;
		const info = await remoteAgent.trust(identity.host, identity.fingerprint);
		setAccount({ ...account, info });
		setIdentity(null);
		setConfirmation("");
		setWorkspace(null);
		setSessions([]);
		chooseSession(null);
		setWorkspaces(
			supports(info.capabilities, "listWorkspaces")
				? await remoteAgent.workspaces(identity.host)
				: [],
		);
	}
	async function selectWorkspace(value: WorkspaceInfo) {
		setWorkspace(value);
		chooseSession(null);
		setSessions(
			supports(capabilities, "listSessions")
				? await remoteAgent.sessions(value.workspace)
				: [],
		);
	}
	async function createSession() {
		if (!workspace) return;
		const outcome = await mutate(
			{ type: "createSession", workspace: workspace.workspace, title: null },
			workspaceKey(workspace.workspace),
		);
		if (
			outcome.status === "succeeded" &&
			outcome.result.type === "sessionCreated"
		) {
			chooseSession(outcome.result.session);
			setSessions(await remoteAgent.sessions(workspace.workspace));
		}
	}
	async function send() {
		if (
			!session ||
			blocked ||
			(!draft.trim() && attachments.length === 0 && !pendingRegion)
		)
			return;
		const capturedSession = session;
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

	return (
		<section
			className="flex h-full min-h-0 flex-col"
			aria-label={t("remote.title")}
		>
			<div className="flex min-h-12 shrink-0 items-center gap-2 border-b px-3">
				<strong className="min-w-0 flex-1 truncate">{t("remote.title")}</strong>
				{busy && (
					<LoaderCircle
						className="size-4 animate-spin"
						aria-label={t("remote.loading")}
					/>
				)}
				{account && (
					<Button
						className="min-h-11 min-w-11"
						variant="ghost"
						disabled={busy}
						onClick={() => void run(refresh)}
						aria-label={t("remote.refresh")}
					>
						<RefreshCw className="size-4" />
					</Button>
				)}
			</div>
			<div className="min-h-0 flex-1 overflow-auto">
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
					<div className="mx-auto flex max-w-md flex-col gap-4 p-4">
						<label className="grid gap-1 text-sm">
							{t("remote.provider")}
							<select
								className="min-h-12 rounded-md border bg-background px-3"
								value={providerId}
								onChange={(event) => setProviderId(event.target.value)}
								disabled={busy}
							>
								{catalog?.providers.map((provider) => (
									<option key={provider.providerId} value={provider.providerId}>
										{provider.displayName}
									</option>
								))}
							</select>
						</label>
						{providerId === "ds-harness-remote" ? (
							<DshRemoteSignIn
								mobile
								onCatalog={adoptCatalog}
								onSignedIn={(value) => run(() => signedIn(value))}
							/>
						) : (
							<p className="text-muted-foreground text-sm">
								{t("remote.providerSetup")}
							</p>
						)}
					</div>
				) : (
					<>
						<div className="flex flex-wrap items-center gap-2 border-b p-3 text-sm">
							<div className="min-w-0 flex-1">
								<p className="truncate">{account.account}</p>
								<p className="text-muted-foreground">
									{account.loginProvider
										? t("remote.signedInWith", {
												provider: account.loginProvider,
											})
										: null}
									{account.loginProvider ? " · " : null}
									{t(
										account.persistence === "systemStore"
											? "remote.secureSignIn"
											: "remote.launchOnly",
									)}
								</p>
							</div>
							<Button
								variant="outline"
								className="min-h-11"
								disabled={busy}
								onClick={() => void run(clearSignIn)}
							>
								{t("remote.logout")}
							</Button>
						</div>
						{!session ? (
							<div className="grid gap-4 p-4 md:grid-cols-2">
								<div className="space-y-2">
									<h2 className="font-medium">{t("remote.hosts")}</h2>
									{hosts.length === 0 && (
										<p className="text-muted-foreground text-sm">
											{t("remote.noHosts")}
										</p>
									)}
									{hosts.map((host) => (
										<Button
											key={host.host.hostId}
											variant="outline"
											className="min-h-12 w-full justify-between"
											disabled={busy || !host.online}
											onClick={() =>
												void run(async () => {
													setConfirmation("");
													setIdentity(await remoteAgent.identity(host.host));
												})
											}
										>
											<span className="truncate">{host.name}</span>
											<span className="text-muted-foreground">
												{t(host.online ? "remote.online" : "remote.offline")}
											</span>
										</Button>
									))}
									{workspaces.length > 0 && (
										<h2 className="pt-3 font-medium">
											{t("remote.workspaces")}
										</h2>
									)}
									{workspaces.map((value) => (
										<Button
											key={JSON.stringify(value.workspace)}
											variant={workspace === value ? "secondary" : "outline"}
											className="min-h-12 w-full justify-start"
											disabled={busy}
											onClick={() => void run(() => selectWorkspace(value))}
										>
											{value.name}
										</Button>
									))}
								</div>
								{workspace && (
									<div className="space-y-2">
										<div className="flex items-center gap-2">
											<h2 className="flex-1 font-medium">
												{t("remote.tasks")}
											</h2>
											{supports(capabilities, "createSession") && (
												<Button
													variant="outline"
													className="min-h-11 min-w-11"
													disabled={
														busy ||
														!journalReady ||
														journal.blocked(workspaceKey(workspace.workspace))
													}
													aria-label={t("remote.newSession")}
													onClick={() => void run(createSession)}
												>
													<Plus className="size-4" />
												</Button>
											)}
										</div>
										{sessions.length === 0 && (
											<p className="text-muted-foreground text-sm">
												{t("remote.noSessions")}
											</p>
										)}
										{sessions.map((value) => (
											<Button
												key={sessionKey(value.session)}
												variant="outline"
												className="h-auto min-h-12 w-full justify-between gap-2 py-3 text-left"
												disabled={
													busy || !supports(capabilities, "readSession")
												}
												onClick={() => chooseSession(value.session)}
											>
												<span className="truncate">
													{value.title || t("remote.untitled")}
												</span>
												<span className="text-muted-foreground text-xs">
													{t(`remote.status.${value.status}`)}
												</span>
											</Button>
										))}
									</div>
								)}
							</div>
						) : (
							<div className="space-y-4 p-4">
								<div className="flex items-center gap-2">
									<Button
										className="min-h-11"
										variant="outline"
										disabled={busy}
										onClick={() => chooseSession(null)}
									>
										{t("remote.tasks")}
									</Button>
									<p className="min-w-0 flex-1 truncate">
										{snapshot?.info.title || t("remote.untitled")}
									</p>
									{snapshot && (
										<span className="text-muted-foreground text-sm">
											{t(`remote.status.${snapshot.info.status}`)}
										</span>
									)}
								</div>
								{loading && (
									<p className="text-muted-foreground">{t("remote.loading")}</p>
								)}
								{snapshot?.events.map((event) => (
									<div
										key={JSON.stringify([event.sourceStreamId, event.sourceId])}
									>
										{event.payload.type === "message" && (
											<Message
												from={
													event.payload.role === "user" ? "user" : "assistant"
												}
												className="max-w-full"
											>
												<MessageContent className="text-base leading-6">
													<MessageResponse>
														{event.payload.text}
													</MessageResponse>
												</MessageContent>
											</Message>
										)}
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
																	if (
																		event.payload.type !== "permissionRequest"
																	)
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
														if (event.payload.type !== "questionRequest")
															return;
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
									</div>
								))}
							</div>
						)}
					</>
				)}
				{account && !journalReady && (
					<div className="flex items-center gap-3 p-3 text-sm" role="status">
						<p className="min-w-0 flex-1 text-muted-foreground">
							{t(
								journalFault
									? "remote.journalUnavailable"
									: "remote.recoveringOperations",
							)}
						</p>
						{journalFault ? (
							<Button
								variant="outline"
								className="min-h-11"
								onClick={() => void restoreOperations()}
							>
								{t("local.retry")}
							</Button>
						) : null}
					</div>
				)}
				{entries.length > 0 && (
					<div className="space-y-2 border-t p-3 text-sm" aria-live="polite">
						{entries.map((entry) => (
							<div key={entry.id} className="rounded-md bg-muted/40 p-2">
								<div className="flex flex-wrap items-center gap-2">
									<code className="min-w-0 flex-1 break-all text-xs">
										{entry.id}
									</code>
									<span>{t(`remote.outcome.${entry.outcome.status}`)}</span>
									{account &&
										supports(capabilities, "operationReceipts") &&
										(entry.outcome.status === "unknown" ||
											entry.outcome.status === "accepted") && (
											<Button
												variant="outline"
												className="min-h-11"
												disabled={busy}
												onClick={() =>
													void run(async () => {
														await inspectOperation(
															entry.id,
															account.info.connection,
														);
													})
												}
											>
												{t("remote.inspectReceipt")}
											</Button>
										)}
								</div>
								{entry.outcome.status === "unknown" && (
									<div className="mt-2 space-y-2">
										<p className="text-muted-foreground">
											{t("remote.unknownOutcome")}
										</p>
										{!journal.isReviewed(entry.id) && (
											<Button
												variant="outline"
												className="min-h-11"
												disabled={
													busy ||
													!(
														entry.target === selectedKey ||
														(workspace &&
															entry.target ===
																workspaceKey(workspace.workspace))
													)
												}
												onClick={() =>
													void run(async () => {
														await refresh();
														if (!regionWorkflow.ownsOperation(entry.id)) {
															setDraft("");
															setAttachments([]);
														}
														journal.allowNewActions(entry.id);
													})
												}
											>
												{t("remote.continueNewTask")}
											</Button>
										)}
									</div>
								)}
							</div>
						))}
					</div>
				)}
			</div>
			{(session || pendingRegion) && (
				<form
					className="shrink-0 space-y-2 border-t bg-background p-3"
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
						value={draft}
						onChange={(event) => setDraft(event.target.value)}
						placeholder={t("remote.message")}
						aria-label={t("remote.message")}
						className="max-h-36 min-h-12 resize-none text-base"
						disabled={
							busy ||
							blocked ||
							!session ||
							Boolean(regionState?.uncertain) ||
							!supports(capabilities, "sendMessage")
						}
					/>
					<div className="flex items-center justify-end gap-2">
						{session && canUpload(capabilities) && (
							<Button
								type="button"
								variant="outline"
								className="min-h-12 min-w-12"
								disabled={busy || blocked || Boolean(regionState?.uncertain)}
								aria-label={t("remote.uploadPrivate")}
								title={t("remote.uploadPrivate")}
								onClick={() => void run(upload)}
							>
								<Paperclip className="size-4" />
							</Button>
						)}
						{session && supports(capabilities, "cancel") && (
							<Button
								type="button"
								variant="outline"
								className="min-h-12 min-w-12"
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
							className="min-h-12 min-w-12"
							aria-label={t("remote.send")}
							disabled={
								busy ||
								blocked ||
								!session ||
								regionBlocked ||
								!supports(capabilities, "sendMessage") ||
								(!draft.trim() && !attachments.length && !pendingRegion)
							}
						>
							<Send className="size-4" />
						</Button>
					</div>
				</form>
			)}
			<Dialog
				open={identity !== null}
				onOpenChange={(value) => {
					if (!value && !busy) {
						setIdentity(null);
						setConfirmation("");
					}
				}}
			>
				<DialogContent>
					<DialogHeader>
						<DialogTitle>{t("remote.trustTitle")}</DialogTitle>
						<DialogDescription>
							{t(
								account?.persistence === "systemStore"
									? "remote.trustDescriptionSecure"
									: "remote.trustDescription",
							)}
						</DialogDescription>
					</DialogHeader>
					<p className="break-words">{identity?.name}</p>
					<code className="select-text break-all rounded bg-muted p-3 text-sm">
						{identity?.fingerprint}
					</code>
					<label
						htmlFor={`${formId}-fingerprint`}
						className="grid gap-2 text-sm"
					>
						{t("remote.confirmFingerprint")}
						<Input
							id={`${formId}-fingerprint`}
							className="min-h-12"
							value={confirmation}
							onChange={(event) => setConfirmation(event.target.value)}
							autoComplete="off"
							autoCapitalize="none"
							disabled={busy}
						/>
					</label>
					<DialogFooter>
						<Button
							variant="outline"
							className="min-h-12"
							disabled={busy}
							onClick={() => setIdentity(null)}
						>
							{t("remote.reject")}
						</Button>
						<Button
							className="min-h-12"
							disabled={
								busy ||
								!identity ||
								!validTrustConfirmation(identity, confirmation)
							}
							onClick={() => void run(trustHost)}
						>
							{t(
								account?.persistence === "systemStore"
									? "remote.trustConnectSecure"
									: "remote.trustConnect",
							)}
						</Button>
					</DialogFooter>
				</DialogContent>
			</Dialog>
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
