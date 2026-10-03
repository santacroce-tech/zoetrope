// Plays what the core's runtime says should be heard. The core decides
// *what* plays *when* (stream positions, event triggers); this module only
// decodes and schedules it with WebAudio. Shared by the editor preview and,
// later, the exported player.
import type { Engine, SoundCue } from "./engine";

/** Restart a stream if it drifts further than this from the timeline (seconds). */
const MAX_DRIFT = 0.12;

interface Playing {
  source: AudioBufferSourceNode;
  /** AudioContext time at which clip position 0 would have played. */
  origin: number;
}

let shared: AudioContext | null = null;

/** Decodes a clip to find its duration (and prove it playable) before import. */
export async function probeAudio(bytes: Uint8Array): Promise<number> {
  shared ??= new AudioContext();
  const buffer = await shared.decodeAudioData(bytes.slice().buffer);
  return buffer.duration;
}

export class AudioEngine {
  private ctx: AudioContext;
  private buffers = new Map<number, AudioBuffer>();
  private pending = new Map<number, Promise<void>>();
  private streams = new Map<string, Playing>();
  private oneShots = new Set<AudioBufferSourceNode>();

  constructor(private engine: Engine) {
    shared ??= new AudioContext();
    this.ctx = shared;
  }

  /** Decodes every audio asset that isn't decoded yet (call before playing). */
  async preload(assetIds: number[]): Promise<void> {
    if (this.ctx.state === "suspended") await this.ctx.resume();
    await Promise.all(assetIds.map((id) => this.load(id)));
  }

  private load(id: number): Promise<void> {
    if (this.buffers.has(id)) return Promise.resolve();
    let p = this.pending.get(id);
    if (!p) {
      const bytes = this.engine.assetBytes(id);
      p = bytes
        ? this.ctx
            .decodeAudioData(bytes.slice().buffer)
            .then((b) => void this.buffers.set(id, b))
            .catch((e) => console.warn(`could not decode audio asset ${id}`, e))
        : Promise.resolve();
      this.pending.set(id, p);
    }
    return p;
  }

  /** Applies the runtime's current sound state. */
  sync(state: { streams: SoundCue[]; events: SoundCue[] }) {
    const now = this.ctx.currentTime;
    const wanted = new Set<string>();
    for (const cue of state.streams) {
      const buffer = this.buffers.get(cue.asset);
      if (!buffer || cue.position >= buffer.duration) continue;
      wanted.add(cue.key);
      const playing = this.streams.get(cue.key);
      if (playing && Math.abs(now - playing.origin - cue.position) <= MAX_DRIFT) continue;
      playing?.source.stop();
      const source = this.start(buffer, cue.volume, cue.position, 0);
      this.streams.set(cue.key, { source, origin: now - cue.position });
    }
    for (const [key, playing] of this.streams) {
      if (!wanted.has(key)) {
        playing.source.stop();
        this.streams.delete(key);
      }
    }
    for (const cue of state.events) {
      const buffer = this.buffers.get(cue.asset);
      if (!buffer) continue;
      const source = this.start(buffer, cue.volume, 0, cue.loops);
      this.oneShots.add(source);
      source.onended = () => this.oneShots.delete(source);
    }
  }

  private start(buffer: AudioBuffer, volume: number, offset: number, loops: number): AudioBufferSourceNode {
    const source = this.ctx.createBufferSource();
    source.buffer = buffer;
    const gain = this.ctx.createGain();
    gain.gain.value = volume;
    source.connect(gain).connect(this.ctx.destination);
    if (loops > 0) {
      source.loop = true;
      source.start(0, offset, buffer.duration * (loops + 1) - offset);
    } else {
      source.start(0, offset);
    }
    return source;
  }

  stopAll() {
    for (const p of this.streams.values()) p.source.stop();
    for (const s of this.oneShots) s.stop();
    this.streams.clear();
    this.oneShots.clear();
  }
}
