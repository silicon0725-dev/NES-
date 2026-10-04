// NES 2.0 S17.4 real extension: proximity alarm camera shake.
// ASCII only, per repo discipline. Cross-game by CONFIG: the node names
// below match BOTH sample games (Dodge and Mini Dungeon ship with nodes
// "player" / "e1".."e3" / "cam"); missing nodes make nes.scene.find
// return null and every pass is a no-op, so the extension also loads
// harmlessly into scenes without them (e.g. the editor demo scene).
//
// Behavior:
//   - nes.onUpdate registers ONE generator coroutine (S17.3 C4): an
//     infinite monitor loop, advanced by the host frame driver.
//   - Each pass reads the tick-end snapshot and computes the minimum
//     player-to-enemy distance. While armed, min distance < triggerDist
//     fires the alarm ONCE: a generator shake of `duration` iterations
//     (each iteration writes a random small offset around the recorded
//     camera base via nes.node.setPos, then `yield 1` = one paused frame
//     per the coroutine contract), then restores the base exactly.
//   - Hysteresis: the alarm re-arms only after distance recovers above
//     rearmDist, so a lingering threat cannot retrigger every frame.
//   - Camera base is captured once (first getPos of the cam node) and
//     never mutated by the shake itself: every offset is base + jitter
//     and the restore lands exactly on base (games may start the camera
//     anywhere; nothing here assumes 192,108).
//   - Optional audio: best-effort nes.audio.play of a CONFIG key. If the
//     mixer has no such key registered the engine drops it silently;
//     a permission denial is caught and swallowed (games ship without
//     audio assets -- sound is optional by design).
//
// Contract used (P0 frozen surface + S17.3 coroutine semantics + S17.5 C6):
//   nes.registerExtension(id)       announce this extension
//   nes.onUpdate(fn)                fn may be a generator function
//   nes.scene.find(name)            -> node ref (opaque number) or null
//   nes.node.getPos(ref)            -> [x, y] (tick-end snapshot)
//   nes.node.setPos(ref, x, y)      queued write, lands same frame
//   nes.audio.play(key, volume)     silent drop on unregistered key
//   nes.util.dist(x1, y1, x2, y2)   Euclidean distance (S17.5 dogfooding:
//                                   the hand-rolled Math.sqrt below was the
//                                   motivating pain point for nes.util)
nes.registerExtension("shake");

var CONFIG = {
  player: "player",
  enemies: ["e1", "e2", "e3"],
  cam: "cam",
  triggerDist: 48,
  rearmDist: 64,
  duration: 20,
  magnitude: 3,
  sound: "Audio/beep",
  volume: 0.5
};

nes.onUpdate(function* () {
  var base = null;
  var armed = true;
  while (true) {
    var pref = nes.scene.find(CONFIG.player);
    var cref = nes.scene.find(CONFIG.cam);
    if (pref === null || cref === null) {
      yield 1;
      continue;
    }
    if (base === null) {
      base = nes.node.getPos(cref);
      if (base === null) {
        yield 1;
        continue;
      }
    }
    var pp = nes.node.getPos(pref);
    var minD = null;
    if (pp !== null) {
      for (var i = 0; i < CONFIG.enemies.length; i++) {
        var eref = nes.scene.find(CONFIG.enemies[i]);
        if (eref === null) {
          continue;
        }
        var ep = nes.node.getPos(eref);
        if (ep === null) {
          continue;
        }
        var d = nes.util.dist(pp[0], pp[1], ep[0], ep[1]);
        if (minD === null || d < minD) {
          minD = d;
        }
      }
    }
    if (minD === null) {
      yield 1;
      continue;
    }
    if (armed && minD < CONFIG.triggerDist) {
      armed = false;
      try {
        nes.audio.play(CONFIG.sound, CONFIG.volume);
      } catch (e) {
        // audio capability not granted in this host: stay silent.
      }
      for (var s = 0; s < CONFIG.duration; s++) {
        var ox = (Math.random() * 2.0 - 1.0) * CONFIG.magnitude;
        var oy = (Math.random() * 2.0 - 1.0) * CONFIG.magnitude;
        nes.node.setPos(cref, base[0] + ox, base[1] + oy);
        yield 1;
      }
      nes.node.setPos(cref, base[0], base[1]);
    } else if (!armed && minD > CONFIG.rearmDist) {
      armed = true;
    }
    yield 1;
  }
});
