import {
	Check,
	ChevronDown,
	Monitor,
	RefreshCw,
	ShieldCheck,
} from "lucide-react";
import {
	type ReactNode,
	useCallback,
	useEffect,
	useRef,
	useState,
} from "react";
import { useTranslation } from "react-i18next";
import { DshRemoteSignIn } from "@/components/remote-agent/dsh-remote-sign-in";
import { Button } from "@/components/ui/button";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuSeparator,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type {
	ConnectionInfo,
	ConnectionRef,
	HostInfo,
	RemoteAgentAccount,
	RemoteAgentCatalog,
	RemoteAgentHostIdentity,
} from "@/lib/core/bindings";
import { errorText } from "@/lib/core/error";
import { notifyError } from "@/lib/core/notify";
import { isMobileApp } from "@/lib/core/tauri";
import { cn } from "@/lib/core/utils";
import {
	canRetrySavedSignIn,
	credentialMessageKey,
} from "@/lib/remote-agent/credentials";
import { remoteAgent } from "@/lib/remote-agent/service";

function sameConnection(left?: ConnectionRef, right?: ConnectionRef) {
	return (
		left?.providerId === right?.providerId &&
		left?.connectionId === right?.connectionId
	);
}

export function useRemoteConnection(
	enabled: boolean,
	active = enabled,
): {
	controls: ReactNode;
	setup: ReactNode;
	host: HostInfo | null;
	connection: ConnectionInfo | null;
} {
	const { t } = useTranslation("mobile");
	const mobile = isMobileApp();
	const [catalog, setCatalog] = useState<RemoteAgentCatalog | null>(null);
	const catalogRef = useRef<RemoteAgentCatalog | null>(null);
	const [hosts, setHosts] = useState<HostInfo[]>([]);
	const [host, setHost] = useState<HostInfo | null>(null);
	const [pending, setPending] = useState<{
		identity: RemoteAgentHostIdentity;
		host: HostInfo;
	} | null>(null);
	const identity = pending?.identity;
	const [busy, setBusy] = useState(false);
	const busyRef = useRef(false);
	const account = catalog?.account;
	const selectedHostId = identity?.host.hostId ?? host?.host.hostId;
	const selectedHostName = identity?.name ?? host?.name;
	const credentialMessage = catalog
		? credentialMessageKey(catalog.credentialState)
		: null;

	const adoptCatalog = useCallback((value: RemoteAgentCatalog) => {
		const previous = catalogRef.current?.account;
		const account = value.account;
		if (!sameConnection(previous?.info.connection, account?.info.connection)) {
			setHosts([]);
			setHost(null);
			setPending(null);
		}
		const next =
			account &&
			previous &&
			JSON.stringify(previous.info) === JSON.stringify(account.info)
				? { ...value, account: { ...account, info: previous.info } }
				: value;
		catalogRef.current = next;
		setCatalog(next);
	}, []);

	const refreshHosts = useCallback(async (connection: ConnectionRef) => {
		const rows = await remoteAgent.hosts(connection);
		if (
			sameConnection(catalogRef.current?.account?.info.connection, connection)
		) {
			setHosts(rows);
			setHost((current) => {
				if (!current) return null;
				const next = rows.find(
					(row) => row.host.hostId === current.host.hostId,
				);
				if (!next?.pinned || next.online === false) return null;
				return JSON.stringify(current) === JSON.stringify(next)
					? current
					: next;
			});
		}
		return rows;
	}, []);

	const reload = useCallback(async () => {
		const previous = catalogRef.current;
		const value = await remoteAgent.catalog();
		// OAuth can activate an account while this catalog read is in flight.
		if (catalogRef.current === previous) adoptCatalog(value);
		const account = catalogRef.current?.account;
		if (account) await refreshHosts(account.info.connection);
	}, [adoptCatalog, refreshHosts]);

	const run = useCallback(
		async (action: () => Promise<unknown>) => {
			if (busyRef.current) return;
			busyRef.current = true;
			setBusy(true);
			try {
				await action();
			} catch (error) {
				notifyError(errorText(error));
				try {
					adoptCatalog(await remoteAgent.catalog());
				} catch {
					/* Keep the last observed account if IPC is unavailable. */
				}
			} finally {
				busyRef.current = false;
				setBusy(false);
			}
		},
		[adoptCatalog],
	);

	useEffect(() => {
		if (!enabled || !active) return;
		const refresh = () => void run(reload);
		refresh();
		// Settings and the Agent panel share the native account across windows.
		window.addEventListener("focus", refresh);
		return () => window.removeEventListener("focus", refresh);
	}, [enabled, active, reload, run]);

	function signInCatalog(value: RemoteAgentCatalog) {
		adoptCatalog(value);
		if (value.account) {
			const connection = value.account.info.connection;
			void run(() => refreshHosts(connection));
		}
	}

	async function signedIn(value: RemoteAgentAccount) {
		const current = catalogRef.current;
		if (!current) return;
		adoptCatalog({
			...current,
			account: value,
			credentialState: "ready",
			persistence: value.persistence,
			canClearSavedSignIn: value.persistence === "systemStore",
		});
		await run(() => refreshHosts(value.info.connection));
	}

	async function connectHost(
		value: RemoteAgentHostIdentity,
		selected: HostInfo,
	) {
		const info = await remoteAgent.trust(value.host, value.fingerprint);
		const current = catalogRef.current;
		if (
			!current?.account ||
			!sameConnection(current.account.info.connection, info.connection)
		)
			return;
		adoptCatalog({ ...current, account: { ...current.account, info } });
		const connected = {
			...selected,
			host: value.host,
			name: value.name,
			pinned: true,
		};
		let confirmed: HostInfo | null = connected;
		try {
			const rows = await refreshHosts(info.connection);
			const latest = rows.find((row) => row.host.hostId === value.host.hostId);
			confirmed = latest?.pinned && latest.online !== false ? latest : null;
		} finally {
			// Keep the verified host if fetching its updated listing fails.
			if (
				sameConnection(
					catalogRef.current?.account?.info.connection,
					info.connection,
				)
			) {
				setHost(confirmed);
				if (confirmed === connected)
					setHosts((rows) =>
						rows.map((row) =>
							row.host.hostId === value.host.hostId ? connected : row,
						),
					);
				setPending(null);
			}
		}
	}

	async function selectHost(selected: HostInfo) {
		if (host?.host.hostId === selected.host.hostId) return;
		const value = await remoteAgent.identity(selected.host);
		if (
			!sameConnection(
				catalogRef.current?.account?.info.connection,
				value.host.connection,
			)
		)
			return;
		setHost(null);
		setPending({ identity: value, host: selected });
		if (value.pinned) await connectHost(value, selected);
	}

	if (!enabled)
		return { controls: null, setup: null, host: null, connection: null };

	const controls = account ? (
		<DropdownMenu>
			<DropdownMenuTrigger asChild disabled={busy}>
				<Button
					type="button"
					variant="ghost"
					size="sm"
					title={selectedHostName ?? t("remote.hosts")}
					aria-label={t("remote.hosts")}
					className={cn("min-w-0 shrink gap-1 px-1.5", mobile ? "h-11" : "h-7")}
				>
					<Monitor className="size-4 shrink-0" />
					<span className="truncate">
						{selectedHostName ?? t("remote.hosts")}
					</span>
					<ChevronDown className="size-3 shrink-0 opacity-70" />
				</Button>
			</DropdownMenuTrigger>
			<DropdownMenuContent align="start" className="w-60">
				{hosts.map((row) => (
					<DropdownMenuItem
						key={row.host.hostId}
						className={cn("gap-2", mobile && "min-h-11")}
						disabled={row.online === false}
						onSelect={() => void run(() => selectHost(row))}
					>
						<span className="min-w-0 flex-1 truncate">{row.name}</span>
						{row.pinned && (
							<ShieldCheck
								className="size-4 shrink-0 text-emerald-600"
								aria-label={t("remote.trusted")}
							/>
						)}
						<span className="text-muted-foreground text-xs">
							{t(
								row.online === true
									? "remote.online"
									: row.online === false
										? "remote.offline"
										: "remote.presenceUnknown",
							)}
						</span>
						{row.host.hostId === selectedHostId && (
							<Check className="size-4 shrink-0" />
						)}
					</DropdownMenuItem>
				))}
				<DropdownMenuSeparator />
				<DropdownMenuItem
					className={mobile ? "min-h-11" : undefined}
					onSelect={() => void run(reload)}
				>
					<RefreshCw className="size-4" />
					{t("remote.refreshHosts")}
				</DropdownMenuItem>
			</DropdownMenuContent>
		</DropdownMenu>
	) : null;

	const setup = (
		<>
			{catalog && account && credentialMessage && (
				<div className="m-4 space-y-3 rounded-lg border p-3" role="status">
					<p className="text-sm">{t(credentialMessage)}</p>
					<div className="flex flex-wrap gap-2">
						{canRetrySavedSignIn(catalog.credentialState) && (
							<Button
								type="button"
								variant="outline"
								className={mobile ? "min-h-12" : undefined}
								disabled={busy}
								onClick={() => void run(reload)}
							>
								{t("remote.credentials.retry")}
							</Button>
						)}
						{catalog.canClearSavedSignIn && (
							<Button
								type="button"
								variant="outline"
								className={mobile ? "min-h-12" : undefined}
								disabled={busy}
								onClick={() =>
									void run(async () => {
										await remoteAgent.logout();
										await reload();
									})
								}
							>
								{t("remote.credentials.clear")}
							</Button>
						)}
					</div>
				</div>
			)}
			{!account && catalog ? (
				<div className="mx-auto flex w-full max-w-md flex-col gap-4 p-2">
					<DshRemoteSignIn
						mobile={mobile}
						onCatalog={signInCatalog}
						onSignedIn={signedIn}
					/>
				</div>
			) : pending ? (
				<div className="flex flex-col items-center gap-3 p-3 text-center">
					<p className="break-words font-medium">{pending.identity.name}</p>
					<Button
						type="button"
						disabled={busy}
						className={mobile ? "min-h-11" : undefined}
						onClick={() =>
							void run(() => connectHost(pending.identity, pending.host))
						}
					>
						<ShieldCheck className="size-4" />
						{t("remote.trustAndConnect")}
					</Button>
				</div>
			) : !host ? (
				<p
					className="px-3 py-5 text-center text-muted-foreground text-sm"
					role="status"
				>
					{t(
						!catalog || busy
							? "remote.loading"
							: hosts.length
								? "remote.chooseHost"
								: "remote.noHosts",
					)}
				</p>
			) : null}
		</>
	);

	return { controls, setup, host, connection: account?.info ?? null };
}
