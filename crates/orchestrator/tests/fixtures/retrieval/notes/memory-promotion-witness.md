# UserAuthored notes bypass the witness gate

A note pinned with `source: AuthorSource::Cli` (i.e. user typed it
into a chat) is treated as a self-witness and promoted to
`Promoted` immediately. All other sources (`Worker`, `Supervisor`,
`MissionResidual`) require N independent witness events before
`reflection::try_promote` clears them. The shortcut is in
`policy.rs::is_user_authored_shortcut`.
