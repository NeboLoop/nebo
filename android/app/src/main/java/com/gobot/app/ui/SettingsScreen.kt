package com.gobot.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import com.gobot.app.GoBotApplication

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    onBack: () -> Unit
) {
    val authManager = GoBotApplication.instance.authManager
    val currentUser by authManager.currentUser.collectAsState()

    var serverUrl by remember { mutableStateOf(authManager.getServerUrl()) }
    var gatewayUrl by remember { mutableStateOf(authManager.getGatewayUrl() ?: "") }
    var useGateway by remember { mutableStateOf(authManager.isUsingGateway()) }
    var showServerDialog by remember { mutableStateOf(false) }
    var showGatewayDialog by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Settings") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                }
            )
        }
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
        ) {
            // User section
            currentUser?.let { user ->
                ListItem(
                    headlineContent = { Text(user.name) },
                    supportingContent = { Text(user.email) },
                    leadingContent = {
                        Icon(Icons.Default.AccountCircle, contentDescription = null)
                    }
                )
                HorizontalDivider()
            }

            // Connection section header
            ListItem(
                headlineContent = {
                    Text(
                        "Connection",
                        style = MaterialTheme.typography.titleSmall,
                        color = MaterialTheme.colorScheme.primary
                    )
                }
            )

            // Local server URL
            SettingsItem(
                icon = Icons.Default.Computer,
                title = "Local Server",
                subtitle = serverUrl,
                onClick = { showServerDialog = true }
            )

            // Gateway toggle
            ListItem(
                headlineContent = { Text("Use Gateway") },
                supportingContent = { Text("Connect through secure gateway") },
                leadingContent = {
                    Icon(Icons.Default.VpnKey, contentDescription = null)
                },
                trailingContent = {
                    Switch(
                        checked = useGateway,
                        onCheckedChange = {
                            useGateway = it
                            authManager.setUseGateway(it)
                        }
                    )
                }
            )

            // Gateway URL (only if enabled)
            if (useGateway) {
                SettingsItem(
                    icon = Icons.Default.Cloud,
                    title = "Gateway URL",
                    subtitle = gatewayUrl.ifEmpty { "Not configured" },
                    onClick = { showGatewayDialog = true }
                )
            }

            HorizontalDivider()

            // About section
            ListItem(
                headlineContent = {
                    Text(
                        "About",
                        style = MaterialTheme.typography.titleSmall,
                        color = MaterialTheme.colorScheme.primary
                    )
                }
            )

            SettingsItem(
                icon = Icons.Default.Info,
                title = "Version",
                subtitle = "1.0.0"
            )

            SettingsItem(
                icon = Icons.Default.Code,
                title = "GitHub",
                subtitle = "github.com/alminisl/gobot"
            )
        }
    }

    // Server URL dialog
    if (showServerDialog) {
        var tempUrl by remember { mutableStateOf(serverUrl) }

        AlertDialog(
            onDismissRequest = { showServerDialog = false },
            title = { Text("Local Server URL") },
            text = {
                Column {
                    Text(
                        "Enter your GoBot server address",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(16.dp))
                    OutlinedTextField(
                        value = tempUrl,
                        onValueChange = { tempUrl = it },
                        label = { Text("URL") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                        placeholder = { Text("http://192.168.1.x:29875") }
                    )
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        serverUrl = tempUrl
                        authManager.setServerUrl(tempUrl)
                        showServerDialog = false
                    }
                ) {
                    Text("Save")
                }
            },
            dismissButton = {
                TextButton(onClick = { showServerDialog = false }) {
                    Text("Cancel")
                }
            }
        )
    }

    // Gateway URL dialog
    if (showGatewayDialog) {
        var tempUrl by remember { mutableStateOf(gatewayUrl) }
        var tempToken by remember { mutableStateOf(authManager.getGatewayToken() ?: "") }

        AlertDialog(
            onDismissRequest = { showGatewayDialog = false },
            title = { Text("Gateway Settings") },
            text = {
                Column {
                    Text(
                        "Configure secure gateway for remote access",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(16.dp))
                    OutlinedTextField(
                        value = tempUrl,
                        onValueChange = { tempUrl = it },
                        label = { Text("Gateway URL") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                        placeholder = { Text("https://your-gateway.domain.com") }
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = tempToken,
                        onValueChange = { tempToken = it },
                        label = { Text("Access Token") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                        placeholder = { Text("Your gateway access token") }
                    )
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        gatewayUrl = tempUrl
                        authManager.setGatewayUrl(tempUrl)
                        authManager.setGatewayToken(tempToken.ifBlank { null })
                        showGatewayDialog = false
                    }
                ) {
                    Text("Save")
                }
            },
            dismissButton = {
                TextButton(onClick = { showGatewayDialog = false }) {
                    Text("Cancel")
                }
            }
        )
    }
}

@Composable
fun SettingsItem(
    icon: ImageVector,
    title: String,
    subtitle: String,
    onClick: (() -> Unit)? = null
) {
    ListItem(
        headlineContent = { Text(title) },
        supportingContent = { Text(subtitle) },
        leadingContent = {
            Icon(icon, contentDescription = null)
        },
        modifier = if (onClick != null) {
            Modifier.clickable(onClick = onClick)
        } else {
            Modifier
        }
    )
}
