// JavaScript evaluated inside the QuickJS sandbox before any user script.
// It builds the Flash-like object API (docs/SCRIPTING.md) on top of one
// host function, `__host(json) → json`, which forwards to the core's script
// bridge (`zoetrope_core::script`). It holds no game semantics of its own:
// what is on stage, what a jump does and what overlaps is all decided by
// the core.
//
// Host → sandbox entry points: `__scope(obj)` (frame/symbol script scope),
// `__obj(path)`, `__removed(keys)` and `__event(e)`.

export const PRELUDE = String.raw`
"use strict";
(function () {
  var host = globalThis.__host;
  delete globalThis.__host;

  function call(msg) {
    var r = host(JSON.stringify(msg));
    return r === "" ? undefined : JSON.parse(r);
  }
  function report(where, e) {
    host(JSON.stringify({ op: "error", where: where, message: String(e && e.message !== undefined ? e.message : e), stack: e && e.stack ? String(e.stack) : "" }));
  }

  function keyOf(path) {
    return path.map(function (s) { return s[0] + "." + s[1]; }).join("/");
  }

  var targets = new Map(); // key → raw target (user properties & handlers live here)
  var proxies = new Map(); // key → proxy given to scripts

  // Properties the core owns; everything else assigned to an object is a
  // plain script property (variables, handlers…).
  var CORE_PROPS = ["x", "y", "rotation", "scaleX", "scaleY", "alpha", "visible", "text"];

  function props(t) {
    var p = call({ op: "get", path: t.__path });
    return p === null ? undefined : p;
  }
  function timeline(t) {
    return call({ op: "timeline", path: t.__path });
  }
  function frameArg(f, method) {
    if (typeof f === "number") {
      if (!(f >= 1)) throw new RangeError(method + ": frames are numbered from 1");
      return Math.floor(f) - 1;
    }
    if (typeof f === "string") return f;
    throw new TypeError(method + " needs a frame number or a frame label");
  }

  var proto = {};
  CORE_PROPS.forEach(function (name) {
    Object.defineProperty(proto, name, {
      get: function () { var p = props(this); return p === undefined ? undefined : p[name]; },
      set: function (v) {
        var patch = {};
        if (name === "visible") v = !!v;
        else if (name === "text") v = String(v);
        else if (typeof v !== "number" || !isFinite(v)) throw new TypeError(name + " must be a finite number");
        patch[name] = v;
        call({ op: "set", path: this.__path, props: patch });
      },
    });
  });
  function getter(name, fn) {
    Object.defineProperty(proto, name, { get: fn });
  }
  getter("name", function () { var p = props(this); return p && p.name; });
  getter("kind", function () { var p = props(this); return p && p.kind; });
  getter("symbolName", function () { var p = props(this); return p && p.symbol; });
  getter("currentFrame", function () { var t = timeline(this); return t ? t.frame + 1 : undefined; });
  getter("totalFrames", function () { var t = timeline(this); return t ? t.length : undefined; });
  getter("isPlaying", function () { var t = timeline(this); return t ? t.playing : false; });
  getter("parent", function () { return this.__path.length ? wrap(this.__path.slice(0, -1)) : null; });
  getter("root", function () { return wrap([]); });
  getter("onStage", function () { return this.__path.length === 0 || props(this) !== undefined; });

  proto.play = function () { call({ op: "play", path: this.__path }); };
  proto.stop = function () { call({ op: "stop", path: this.__path }); };
  proto.gotoAndPlay = function (f) { call({ op: "goto", path: this.__path, frame: frameArg(f, "gotoAndPlay"), play: true }); };
  proto.gotoAndStop = function (f) { call({ op: "goto", path: this.__path, frame: frameArg(f, "gotoAndStop"), play: false }); };
  proto.nextFrame = function () {
    var t = timeline(this);
    if (t && t.frame + 1 < t.length) call({ op: "goto", path: this.__path, frame: t.frame + 1, play: false });
  };
  proto.prevFrame = function () {
    var t = timeline(this);
    if (t && t.frame > 0) call({ op: "goto", path: this.__path, frame: t.frame - 1, play: false });
  };
  proto.getChildByName = function (name) { return wrap(call({ op: "child", path: this.__path, name: String(name) })); };
  proto.getChildren = function () {
    return call({ op: "children", path: this.__path }).map(function (c) { return wrap(c.path); });
  };
  proto.getBounds = function () { return call({ op: "bounds", path: this.__path }); };
  proto.hitTestPoint = function (x, y, shapeFlag) {
    return call({ op: "hitTestPoint", path: this.__path, x: Number(x), y: Number(y), shape: !!shapeFlag });
  };
  proto.hitTestObject = function (other) {
    if (!other || !other.__path) throw new TypeError("hitTestObject needs a display object");
    return call({ op: "hitTestObject", path: this.__path, other: other.__path });
  };
  proto.toString = function () { return "[object " + (this.kind || "DisplayObject") + " " + (this.name || "") + "]"; };

  var handler = {
    get: function (t, k, receiver) {
      if (typeof k !== "string" || k in t) return Reflect.get(t, k, receiver);
      // Unset event handlers are just undefined (no lookup).
      if (/^on[A-Z]/.test(k)) return undefined;
      // Otherwise: a named child, like Flash's instance names.
      var child = call({ op: "child", path: t.__path, name: k });
      return child === null ? undefined : wrap(child);
    },
  };

  function wrap(path) {
    if (path === null || path === undefined) return null;
    var key = keyOf(path);
    var p = proxies.get(key);
    if (p) return p;
    var t = Object.create(proto);
    Object.defineProperty(t, "__path", { value: path });
    Object.defineProperty(t, "__key", { value: key });
    p = new Proxy(t, handler);
    targets.set(key, t);
    proxies.set(key, p);
    return p;
  }

  // ---------------------------------------------------------------- globals

  var keys = new Set();
  var listeners = {};
  globalThis.Key = Object.freeze({
    /** Whether a key is held: KeyboardEvent.key ("ArrowLeft", "a", " ") or .code ("KeyA", "Space"). */
    isDown: function (k) { return keys.has(String(k)); },
  });
  var mouse = { x: 0, y: 0, isDown: false };
  globalThis.Mouse = Object.freeze({
    get x() { return mouse.x; },
    get y() { return mouse.y; },
    get isDown() { return mouse.isDown; },
  });
  var stageInfo = call({ op: "stageInfo" });
  globalThis.stage = Object.freeze({
    width: stageInfo.width,
    height: stageInfo.height,
    fps: stageInfo.fps,
    addEventListener: function (type, fn) {
      if (typeof fn !== "function") throw new TypeError("addEventListener needs a function");
      (listeners[type] = listeners[type] || []).push(fn);
    },
    removeEventListener: function (type, fn) {
      var l = listeners[type];
      if (l) listeners[type] = l.filter(function (g) { return g !== fn; });
    },
  });
  Object.defineProperty(globalThis, "root", { get: function () { return wrap([]); } });
  globalThis.trace = function () {
    host(JSON.stringify({ op: "trace", text: Array.prototype.map.call(arguments, String).join(" ") }));
  };

  // ---------------------------------------------------------------- host entry points

  var BOUND = ["play", "stop", "gotoAndPlay", "gotoAndStop", "nextFrame", "prevFrame", "getChildByName", "getChildren", "getBounds", "hitTestPoint", "hitTestObject"];

  // The scope of a frame or symbol script: free names resolve against the
  // timeline object, as in Flash. 'var' declarations become its properties
  // (shared by its frame scripts); 'let'/'const'/'function' stay local.
  globalThis.__scope = function (self) {
    return new Proxy(Object.create(null), {
      has: function (_, k) { return typeof k === "string" && k !== "arguments" && !(k in globalThis); },
      get: function (_, k) {
        if (typeof k !== "string") return undefined;
        var v = self[k];
        return typeof v === "function" && BOUND.indexOf(k) >= 0 ? v.bind(self) : v;
      },
      set: function (_, k, v) { self[k] = v; return true; },
    });
  };

  globalThis.__obj = function (path) { return wrap(path); };

  globalThis.__removed = function (removedKeys) {
    removedKeys.forEach(function (key) {
      Array.from(proxies.keys()).forEach(function (k) {
        if (k === key || k.indexOf(key + "/") === 0) {
          proxies.delete(k);
          targets.delete(k);
        }
      });
    });
  };

  var BUTTON_HANDLERS = { press: "onPress", release: "onRelease", click: "onClick" };

  function fire(where, fn, self, arg) {
    try {
      fn.call(self, arg);
    } catch (e) {
      report(where, e);
    }
  }
  function fireStage(type, arg) {
    (listeners[type] || []).slice().forEach(function (fn) { fire("stage " + type + " listener", fn, globalThis.stage, arg); });
  }

  globalThis.__event = function (e) {
    switch (e.type) {
      case "enterFrame":
        // Every object with an onEnterFrame handler, oldest first.
        Array.from(targets.entries()).forEach(function (entry) {
          var t = entry[1];
          if (typeof t.onEnterFrame === "function" && targets.get(entry[0]) === t) {
            fire("onEnterFrame of " + (entry[0] ? (t.name || entry[0]) : "root"), t.onEnterFrame, proxies.get(entry[0]));
          }
        });
        fireStage("enterFrame");
        break;
      case "button": {
        var name = BUTTON_HANDLERS[e.event];
        var p = wrap(e.path);
        var t = targets.get(keyOf(e.path));
        if (name && t && typeof t[name] === "function") fire(name + " of " + (e.name || "button"), t[name], p);
        fireStage(e.event === "click" ? "click" : e.event === "press" ? "buttonPress" : "buttonRelease", p);
        break;
      }
      case "keyDown":
      case "keyUp":
        if (e.type === "keyDown") { keys.add(e.key); keys.add(e.code); }
        else { keys.delete(e.key); keys.delete(e.code); }
        fireStage(e.type, { key: e.key, code: e.code });
        break;
      case "blur":
        keys.clear();
        break;
      case "mouseMove":
      case "mouseDown":
      case "mouseUp":
        mouse.x = e.x;
        mouse.y = e.y;
        if (e.type !== "mouseMove") mouse.isDown = e.type === "mouseDown";
        fireStage(e.type, { x: e.x, y: e.y });
        break;
    }
  };
})();
`;
