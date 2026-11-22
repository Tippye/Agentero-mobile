package app.agentero.mobile

/** Sync and transfer leases share a bounded dataSync carrier, separate from Agent. */
class AndroidDataSyncService : AndroidBackgroundService(BackgroundLane.SYNC) {
    override fun onTimeout(startId: Int, fgsType: Int) {
        // Stop within Android's deadline; Rust receives a checkpoint/cancellation signal.
        AndroidBackgroundController.get(this).timeout()
        finish()
    }
}
