thin=min(dx,dy,dz); big=max(dx,dy,dz)
tall_z=(dz>=dx and dz>=dy)
is_flat=(thin < 0.5*big)
axis=None
if is_flat and not tall_z:
 axis=(1,0,0) if (dy<=dx and dy<=dz) else ((0,1,0) if (dx<=dy and dx<=dz) else None)
if axis:
 mat=oc.transform2.copy(); r=adsk.core.Matrix3D.create()
 r.setToRotation(math.pi/2, adsk.core.Vector3D.create(*axis), adsk.core.Point3D.create(0,0,0))
 mat.transformBy(r); oc.transform2=mat
