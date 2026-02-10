package com.akily.p2pshare.storage

import android.content.Context
import android.net.Uri

class PrefsStore(context: Context) {
    private val prefs = context.getSharedPreferences("p2p_share_prefs", Context.MODE_PRIVATE)

    fun saveOutputTree(uri: Uri?) {
        prefs.edit().putString(KEY_OUTPUT_TREE, uri?.toString()).apply()
    }

    fun loadOutputTree(): Uri? {
        val raw = prefs.getString(KEY_OUTPUT_TREE, null) ?: return null
        return runCatching { Uri.parse(raw) }.getOrNull()
    }

    companion object {
        private const val KEY_OUTPUT_TREE = "output_tree_uri"
    }
}
