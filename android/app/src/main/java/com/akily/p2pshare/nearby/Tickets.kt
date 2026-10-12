package com.akily.p2pshare.nearby

import android.util.Base64
import org.json.JSONObject

/** Read-only view of a `p2psh:` ticket (base64url JSON, see `crates/core/src/ticket.rs`). */
object Tickets {
    private fun endpoint(ticket: String): JSONObject? = runCatching {
        val data = ticket.trim().substringAfter(':')
        val flags = Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP
        JSONObject(String(Base64.decode(data, flags), Charsets.UTF_8)).getJSONObject("endpoint")
    }.getOrNull()

    fun peerId(ticket: String): String? = endpoint(ticket)?.optString("id")?.takeIf { it.isNotEmpty() }

    /** Whether the peer reached a relay (had internet) when the ticket was made. */
    fun hasRelay(ticket: String): Boolean {
        val addrs = endpoint(ticket)?.optJSONArray("addrs") ?: return false
        return (0 until addrs.length()).any { addrs.optJSONObject(it)?.has("Relay") == true }
    }
}
