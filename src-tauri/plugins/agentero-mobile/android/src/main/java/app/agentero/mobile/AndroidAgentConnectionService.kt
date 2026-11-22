package app.agentero.mobile

/** API 34+ specialUse declaration is limited to user-enabled remote connectivity. */
class AndroidAgentConnectionService : AndroidBackgroundService(BackgroundLane.AGENT)
