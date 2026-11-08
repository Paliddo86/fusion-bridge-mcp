# Electrical priorities and route shape

Start from the actual netlist, component datasheets, stackup, copper thickness, load cases and
board constraints. Ask only for missing information that affects the requested routing decision.
Do not infer current from a net name or a wide-looking existing trace.

Inventory supply/return paths, high-current connectors, switching loops, gate drives, sense
connections, clocks and differential signals. Preserve placements the user did not authorize
changing. Budget voltage drop and resistive loss using path length, cross-section and copper
resistivity at an appropriate temperature. Current capacity also depends on layer, copper weight,
allowed rise, adjacent copper, vias and enclosure; use applicable primary guidance and state
assumptions rather than declaring a current rating from width alone.

Prefer horizontal/vertical paths with 45-degree transitions where routing freedom permits.
Minimize unnecessary corners, acute wedges and tiny segments. Check angles after native readback,
including pad escapes; allow a justified exception when electrical or footprint geometry requires
it. Curves are an option when requested and supported, with controlled radius and native geometry
verification. Aesthetic cleanup must not lengthen critical loops or break impedance constraints.

Reserve compact switching-current loops and an adjacent return path before ordinary signals.
Keep switch-node copper only as large as electrically/thermally justified; do not expand every
power-labeled net indiscriminately. Keep sense lines away from switching copper and route Kelvin
connections to their intended sensing points. Differential impedance, pair spacing, skew and
reference continuity follow the actual interface specification and stackup.

Select via drills, annular rings and counts for fabrication, current, heat and assembly. Include
via barrels in voltage-drop/thermal estimates; do not assume an arbitrary via count shares current
equally. Reuse electrically valid plated endpoints when practical. For every layer change, inspect
the reference plane and return transition. Report longer routes, removed ground copper and other
costs of an alternate path. Reject a relocation that improves appearance or heat spreading while
violating the signal's electrical constraints.
