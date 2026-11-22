package app.agentero.mobile

import app.tauri.annotation.InvokeArg

@InvokeArg
internal class AndroidBackgroundConfigureArgs {
    var enabled: Boolean? = null
}

@InvokeArg
internal class AndroidBackgroundSettingsArgs {
    var target: String? = null
}

@InvokeArg
internal class AndroidBackgroundApplyArgs {
    var generation: String? = null
    var agentCount: Long? = null
    var syncCount: Long? = null
    var transferCount: Long? = null

    fun counts() = BackgroundCounts(
        requireNotNull(agentCount) { "Missing Agent count" },
        requireNotNull(syncCount) { "Missing sync count" },
        requireNotNull(transferCount) { "Missing transfer count" },
    )
}
