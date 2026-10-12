# Pi extensions are outside Pipkin's scope

Pipkin does not load Pi TypeScript extensions, remote extension services, terminal widgets or
extension-contributed commands. `--pi-extension` is retired. The owner explicitly removed Pi extension
compatibility from the native-engine conversion requirements.

Import preserves historical plugin selections and unknown profile bytes with checksums without
executing their code. An extension selection alone does not block conversation import; unreadable,
oversized or changing source profiles still block it to protect data. Native subagents, tools,
provider configuration and built-in desktop interactions remain supported independently of extensions.

The former remote-question contract and Pi implementation are retained in Git through `76ca778`
for provenance. They are not current Pipkin features or unfinished parity requirements.
