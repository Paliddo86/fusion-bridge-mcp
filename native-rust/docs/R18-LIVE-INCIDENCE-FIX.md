# R18 live line incidence correction

The R18-fix1 first line read failed before constraint execution at diagnostic stage432: SketchPoint.connectedEntities returned null, with SDK error0. The failed point was not recorded; the receipt cannot identify it. The campaign stopped, preserved all 20 raw exchanges and three settled CAD effects, and did not invoke a constraint.

The typed shim now derives curve incidence only from a complete inventory: exactly one retained SketchLine; exactly three distinct SketchPoints, its start, end and sketch origin; exact complete point membership; and zero sketch texts. It rechecks complete curve/point membership, the three typed identities and zero texts after getters; the Rust caller still requires two stable captures bracketed by revision guards. The existing line-state contract requires zero dimensions. Constraint relations are checked separately.

`points[].connected` remains curve incidence, with values1 for the two endpoints and0 for the distinct origin; its source is the complete typed inventory. The null SDK getter is never called and null is never interpreted as an empty list. No error clearing, retry, CAD write or broader graph claim is introduced. Coincident coordinates do not establish identity. The portable helper test covers each valid identity and refuses null, aliased and foreign identities.

The installed SketchPoint.h and [Autodesk SketchPoint documentation](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_SketchPoint.htm) distinguish geometry usage from relationships through constraints. This finite proof is not a replacement for general adjacency discovery.

This source correction is not live solver qualification. Only a fresh runtime and new task model may establish that; retired object IDs and failed campaign effects are never replayed.
