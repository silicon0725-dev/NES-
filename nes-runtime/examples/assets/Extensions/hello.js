// NES 2.0 S17 demo extension (ASCII only, per repo discipline).
//
// The FIRST JS extension of the engine: it runs inside the Extension
// Execution Runtime (QuickJS-NG) and may ONLY touch the engine through the
// injected "nes" capability object -- never the SceneTree/Renderer directly.
//
// Contract used here (P0 frozen surface):
//   nes.registerExtension(id)       announce this extension
//   nes.onUpdate(fn)                per-frame hook (called after simulate)
//   nes.scene.find(name)            -> node ref (opaque number) or null
//   nes.node.getPos(ref)            -> [x, y] (tick-end snapshot)
//   nes.node.setPos(ref, x, y)      queued write, lands same frame
//   nes.input.isPressed(name)       -> bool (this frame's input snapshot)
//   nes.audio.play(key, volume)     mixer key = resource path w/o extension
nes.registerExtension("hello");

var angle = 0.0;
var radius = 60.0;
var omega = 0.05;
var center = null;

nes.onUpdate(function () {
  var ref = nes.scene.find("obj1");
  if (ref === null) {
    return;
  }
  if (center === null) {
    center = nes.node.getPos(ref);
  }
  angle = angle + omega;
  var x = center[0] + Math.cos(angle) * radius;
  var y = center[1] + Math.sin(angle) * radius;
  nes.node.setPos(ref, x, y);

  if (nes.input.isPressed("Space")) {
    nes.audio.play("Audio/beep", 0.5);
  }
});
