# Pours, copper retention, and thermal paths

Before allocating copper, inventory hot/high-current devices, exposed pads, thermal holes and
existing heatsink vias. Resolve each pad's electrical net from the board and datasheet: a MOSFET's
large thermal pad is not necessarily ground. Establish loss estimates or label thermal comparisons
as illustrative when losses and enclosure conditions are unknown.

Reserve broad connected copper from thermal pads and useful plated connections, balancing routing
space, assembly and other devices' cooling. Outer-layer spreading can help surface heat rejection;
inner routing can free it, subject to the actual stackup and return planes. Set native polygon net,
layer, rank, clearance, relief and orphan settings explicitly. Refill and inspect fragmentation,
thermals and contact to load pads. A large outline or zero airwires can conceal a thin residual
trace carrying the current. Use disposable diagnostics to establish whether the pour itself
provides the intended electrical path; preserve the accepted board.

After initial routing, expand useful connected regions into permissible remaining area. Measure
refilled copper unions excluding drill holes; do not double-count overlap or floating islands as
connected heatsinking. Account for copper reassigned from ground or another thermal path. Report
coverage and useful net area separately; larger area is not automatically better overall cooling.

Inspect narrow necks and traces that nearly divide a region. If a distant lobe is poorly coupled,
consider rerouting an appropriate obstructing signal, using another layer and existing plated
endpoints or justified new vias. Preserve signal references, width and other constraints. Compare
connectivity, DRC, route length, ground continuity and heat flow before accepting the move. A failed
candidate is discarded, not silently substituted into the reported final plan.

For ablation, maximize retained copper only when that is the user's fabrication objective.
Electrical insulation, edge clearance and keepouts remain binding. Retain floating islands only
when requested and suitable for the circuit/process; do not count them as net-connected heat
paths or default to them for every board. Coverage alone does not establish laser-cycle savings:
contour length, travel and required passes also matter.

For mask choices, compare coating conduction resistance, convection and radiation. Removing mask
adds negligible macroscopic area; low-emissivity bright metal can radiate less than coated copper.
Use actual finish data or a labeled sensitivity range. Treat partial openings spatially or label
an area-weighted approximation. Contact heatsinks/chassis paths require separate electrical
isolation, interface and assembly design. Do not assume exposing copper always improves cooling.

For calculated heat spreading, state source temperature or power boundary, ambient, copper
thickness/conductivity, convection, emissivity, mask properties and included layers. Check mesh
sensitivity, energy balance and neck behavior. Distinguish bottom-sheet heat-flow estimates from
junction temperature, complete-board dissipation and measured results. Include package/via/FR4,
other layers and enclosure effects when the decision requires them; otherwise disclose omissions.

After accepted changes, native-refill and rerun the same rule profile, save the analyzed geometry,
and highlight the actual affected nets when explaining the result. Report tradeoffs and remaining
qualification work instead of calling an illustrative candidate globally optimal.
