# AI Flow Mechanical integration handoff

Checked against `adom/adom-aiflow-mechanical` main commit
`1195dc3f103cbf01a53ffaa85a414ef98be8fe53` on 16 October 2026.
This assessment covers published source, not unpushed work on John's machine.

John's version 0.1.0 prototype is a Rust JSON review CLI with `serde_json` as its
only dependency. Its two included tests passed in a locked build. It checks
fastener inventory, tool, approach, removal sequence and retention; it compares
declared hole diameter against the larger of tool envelope or extracted part,
plus twice the radial clearance. Angled access, swept-volume verification and
board revision verification rely on supplied verification booleans. Its skill
asks for real CAD evidence, but its executable does not yet validate linked
reports or source hashes.

There are no Fusion, SolidWorks, Adom Bridge, SDK, network, subprocess or CAD
import/export calls in the reviewed executable. It is complementary review
policy, not a replacement for the native CAD backend. The parent AIFlow project
has a documented Fusion electronics lane through the existing bridge; that
does not connect this mechanical prototype to the new Rust runtime.

## Proposed division of work

| Layer | Responsibility |
| --- | --- |
| AI Flow Mechanical | Review workflow, fastener/tool selection, approach/removal/retention policy, guidance and findings |
| Fusion bridge | CAD execution, exact model/revision identity, occurrence transforms, geometry, measurements, interference queries and durable execution evidence |
| Evidence adapter, not implemented | Translate verified native observations into a versioned review report with explicit unsupported checks |

The adapter should carry exact board and assembly identities/hashes, units,
occurrence identities, tool envelopes, measured clearances, check methods,
scope, provenance and unresolved checks. A successful CAD command is not proof
of fit. Do not set `sweptVolumeVerified` or `boardRevisionVerified` merely to
make lint pass. Physical imported assemblies, geometry measurements and collision
support are still required before a complete service-access workflow can pass.

Current relevant backend proof is the finite empty-component R20-fix3 assembly
qualification. R21 positioning is in progress; positioning alone does not prove
a removal path is collision-free. General collision and swept-volume checks
remain unimplemented in the native candidate. A second review UI or duplicate
policy engine is unnecessary.

Sources: [AI Flow Mechanical](https://wiki.adom.inc/adom/adom-aiflow-mechanical),
[AIFlow](https://wiki.adom.inc/adom/adom-aiflow), and the
[current bridge progress report](README.md).
