"""Clean captures. A viewport image shows what the graphics window shows, so joint and joint origin
glyphs, sketches, origin planes, axes and points, and construction planes, axes and points appear in
every frame. The capture operations hide them through the design's Object Visibility settings (the
Display Settings > Object Visibility menu; Design.objectVisibility in the API) and put back exactly
the switches they turned off. Fusion's local renderer never draws them, but Viewport.fit() counts
them, so render_image hides them only while it fits a standard view.

Measured on Fusion 2705.1.30 against the other routes:
- On a clean new design, all ten switches off and back on left isModified false. One folder light bulb
  (Component.isJointsFolderLightBulbOn and the rest) set it, and turning the bulb back did not clear it;
  the API cannot clear that flag.
- Object Visibility costs one setter per switch (ten in 0.3 ms) whatever the size of the design. The
  folder light bulbs cost a native call per folder per component: 48 toggles at 6.9 ms each on 23
  components, and a light-bulb toggle costs about 26 ms on a 4,579-occurrence assembly.
- Both routes remove the same pixels (0 differ). Entity light bulbs miss the origin geometry, and
  Design.setLightBulbOn refuses joints."""

# The Object Visibility switches a clean capture turns off, with the names replies use.
SWITCHES = (("isJointsVisible", "joints"), ("isJointOriginsVisible", "jointOrigins"),
            ("isJointOriginAxesVisible", "jointOriginAxes"), ("isSketchesVisible", "sketches"),
            ("isOriginPlanesVisible", "originPlanes"), ("isOriginAxesVisible", "originAxes"),
            ("isOriginPointsVisible", "originPoints"), ("isUserWorkPlanesVisible", "constructionPlanes"),
            ("isUserWorkAxesVisible", "constructionAxes"), ("isUserWorkPointsVisible", "constructionPoints"))
UNAVAILABLE = "This Fusion has no Object Visibility API (Design.objectVisibility), so glyphs show as the design shows them."


class Glyphs:
    """Hide the glyphs a design shows for a capture, then put every switch back as it was.

    show=True (the caller's `glyphs: true`) leaves everything as the design shows it. hide() turns off
    only the switches that are on. restore() never raises and may be called more than once: it runs
    from the capture's `finally` beside the camera and pose restores, and records any switch that did
    not come back so the operation can refuse with restore_failed after its other restores have run."""

    def __init__(self, design, show=False):
        self.design, self.show = design, bool(show)
        self.settings, self.before, self.hidden, self.failed, self.back = None, {}, [], [], False

    def hide(self):
        if self.show:
            return self
        self.settings = getattr(self.design, "objectVisibility", None)
        if self.settings is None:
            return self
        self.before = {attr: bool(getattr(self.settings, attr)) for attr, _ in SWITCHES}
        for attr, name in SWITCHES:
            if self.before[attr]:
                setattr(self.settings, attr, False)
                self.hidden.append((attr, name))
        return self

    def restore(self):
        """Turn back on what hide() turned off, then check every switch against its state before."""
        if not self.hidden or self.back:
            return True
        failed = []
        for attr, name in self.hidden:
            try:
                setattr(self.settings, attr, True)
            except Exception:
                pass
        for attr, name in SWITCHES:
            try:
                if bool(getattr(self.settings, attr)) != self.before[attr]:
                    setattr(self.settings, attr, self.before[attr])
                ok = bool(getattr(self.settings, attr)) == self.before[attr]
            except Exception:
                ok = False
            if not ok:
                failed.append(name)
        self.failed, self.back = failed, not failed
        return self.back

    @property
    def restored(self):
        return not self.failed

    def report(self):
        """The reply's `glyphs`: whether they were left showing, which kinds were hidden, and that every
        switch is back."""
        out = {"shown": self.show or self.settings is None, "hidden": [name for _, name in self.hidden], "restored": not self.failed}
        if not self.show and self.settings is None:
            out["note"] = UNAVAILABLE
        return out
