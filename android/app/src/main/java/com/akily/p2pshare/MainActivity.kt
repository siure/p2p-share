package com.akily.p2pshare

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import com.akily.p2pshare.ui.P2PShareApp
import com.akily.p2pshare.ui.TransferViewModel
import com.akily.p2pshare.ui.theme.P2PShareTheme

class MainActivity : ComponentActivity() {
    private val viewModel: TransferViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            P2PShareTheme {
                P2PShareApp(viewModel)
            }
        }
    }
}
