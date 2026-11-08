thin=min(dx,dy,dz); big=max(dx,dy,dz)
tall_z=(dz>=dx and dz>=dy)
axis=None
if (thin < 0.5*big) and not tall_z:
    axis=(1,0,0) if (dy<=dx and dy<=dz) else ((0,1,0) if (dx<=dy and dx<=dz) else None)
