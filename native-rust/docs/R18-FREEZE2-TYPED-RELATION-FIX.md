# Exact typed relation check, freeze 2

Independent review found that the vertical readback branch retained the returned VerticalConstraint but did not compare a reacquired typed wrapper against it. The horizontal branch already checked its retained typed wrapper. Both branches now use the same exact-holder guard before reading line(). An alternative relation referring to the same line is refused. The generic relation, parent sketch, context and line checks remain in place.

Freeze 1 is archived byte-for-byte under evidence/freeze1-source (145 manifest entries). Only relations.inc, build.rs, the new relation_identity.h and its portable C++ guard test change production/test code. The test exercises both typed branches, a substituted relation on the same line, missing holders, and refusal before any line getter. It uses instrumented stand-ins, not Autodesk objects.

Full Linux workspace: 437 passed including 4 compile-fail doc tests; native suite 260. Portable C++ guard regression passed. Windows GNU test bindings, adapted installed SDK syntax and Rust format checks passed. Actual MSVC/Fusion qualification belongs to the parent operator and is not established by these checks. No runtime behavior beyond this membership refusal changes.
