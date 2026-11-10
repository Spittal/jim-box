import { Channel, invoke } from '@tauri-apps/api/core';
import workletSource from './player.worklet.js?raw';

/** How long the player sits paused before the audio device is let go. */
const IDLE_MS = 30_000;

/** What the audio thread says back. */
type Report = { type: 'at'; frame: number; time: number; seq: number } | { type: 'ended'; seq: number };

let workletUrl: string | null = null;
/** Names each decode to the backend, which carries on with the latest only. */
let streams = Date.now();

/**
 * The player's audio. The backend decodes a track and sends its samples as
 * it goes (see `track_audio`); an audio worklet holds them and plays them.
 * So a track starts on its first fifth of a second, not once it is all
 * decoded, and play, pause and seek are a message to a running audio thread:
 * a few milliseconds, where a media element takes a noticeable moment.
 */
export class TrackAudio {
  /** Called when `ready`, `playing`, `durationMs` or a paused position changes. */
  onstate: () => void = () => {};
  /** Called when the track has played to its end. */
  onended: () => void = () => {};
  /** Called when the audio can't be set up or decoded. */
  onerror: (error: unknown) => void = () => {};

  /** The track can be played and sought. */
  ready = false;
  playing = false;
  /** An estimate until the track is decoded to its end. */
  durationMs = 0;
  context: AudioContext | null = null;

  private node: AudioWorkletNode | null = null;
  /** The track rate `context` was made for. */
  private contextRate = 0;
  private rate = 44_100;
  /** Frames decoded so far, and whether that's all of them. */
  private frames = 0;
  private done = false;
  /** Where playing was, in frames, at `time` on the context's clock. */
  private frame = 0;
  private time = 0;
  /** Counts loads and seeks, so reports from before the latest are dropped. */
  private seq = 0;
  /** The decode being received; 0 for none. */
  private stream = 0;
  private idle = 0;

  /** Loads a track and plays it. Resolves once it is all decoded. */
  async load(trackId: string) {
    this.unload();
    const stream = ++streams;
    this.stream = stream;
    let started = false;
    // Messages are handled one after another, though the first waits on the audio device.
    let handled: Promise<void> = Promise.resolve();
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (data) => {
      handled = handled
        .then(async () => {
          if (stream !== this.stream) return;
          if (!started) {
            started = true;
            await this.start(stream, new DataView(data));
          } else if (data.byteLength) {
            this.frames += data.byteLength / 4;
            this.node?.port.postMessage({ type: 'chunk', data }, [data]);
          } else {
            this.done = true;
            this.durationMs = (this.frames / this.rate) * 1000;
            this.node?.port.postMessage({ type: 'done' });
            this.onstate();
          }
        })
        .catch((e) => {
          if (stream === this.stream) this.onerror(e);
        });
    };
    await invoke('track_audio', { trackId, stream, onAudio: channel });
  }

  /** The first message: the sample rate and about how many frames are coming. */
  private async start(stream: number, header: DataView) {
    const rate = header.getUint32(0, true);
    const frames = header.getUint32(4, true);
    await this.open(rate);
    if (stream !== this.stream || !this.context || !this.node) return;
    this.rate = rate;
    this.durationMs = (frames / rate) * 1000;
    this.node.port.postMessage({ type: 'start', frames, step: rate / this.context.sampleRate, seq: ++this.seq });
    this.ready = true;
    this.play();
  }

  /** An audio device running at the track's rate where it can, so nothing is resampled here. */
  private async open(rate: number) {
    if (this.context && this.contextRate === rate) return;
    this.node?.disconnect();
    this.node = null;
    await this.context?.close();
    let context: AudioContext;
    try {
      context = new AudioContext({ sampleRate: rate, latencyHint: 'interactive' });
    } catch {
      // A rate the device can't be opened at: the worklet steps through the samples instead.
      context = new AudioContext({ latencyHint: 'interactive' });
    }
    workletUrl ??= URL.createObjectURL(new Blob([workletSource], { type: 'text/javascript' }));
    await context.audioWorklet.addModule(workletUrl);
    const node = new AudioWorkletNode(context, 'track-player', { numberOfInputs: 0, outputChannelCount: [2] });
    node.port.onmessage = (event: MessageEvent<Report>) => this.heard(event.data);
    node.connect(context.destination);
    this.context = context;
    this.contextRate = rate;
    this.node = node;
  }

  private heard(report: Report) {
    if (report.seq !== this.seq) return;
    if (report.type === 'ended') {
      this.playing = false;
      this.frame = this.frames;
      this.rest();
      this.onstate();
      this.onended();
      return;
    }
    this.frame = report.frame;
    this.time = report.time;
    // Paused, this is exactly where it stopped.
    if (!this.playing) this.onstate();
  }

  /** Seconds between a sample being played here and being heard. */
  get latency(): number {
    return (this.context?.outputLatency || 0) + (this.context?.baseLatency || 0);
  }

  /** Where the track is being heard. */
  positionMs(): number {
    const at = (this.frame / this.rate) * 1000;
    if (!this.playing || !this.context) return at;
    return at + Math.max(0, this.context.currentTime - this.time - this.latency) * 1000;
  }

  play() {
    if (!this.ready || !this.context || !this.node || this.playing) return;
    clearTimeout(this.idle);
    if (this.context.state !== 'running') void this.context.resume();
    // From the end, play starts over.
    if (this.done && this.frame >= this.frames - 1) this.seek(0);
    this.playing = true;
    this.time = this.context.currentTime;
    this.node.port.postMessage({ type: 'play' });
    this.onstate();
  }

  pause() {
    if (!this.playing) return;
    this.frame = (this.positionMs() / 1000) * this.rate;
    this.playing = false;
    this.node?.port.postMessage({ type: 'pause' });
    this.rest();
    this.onstate();
  }

  seek(ms: number) {
    if (!this.ready || !this.node) return;
    this.frame = Math.max(0, Math.round((ms / 1000) * this.rate));
    this.time = this.context?.currentTime ?? 0;
    this.node.port.postMessage({ type: 'seek', frame: this.frame, seq: ++this.seq });
  }

  /** Lets go of the track: its audio stops and its samples are dropped. */
  unload() {
    if (this.stream) invoke('stop_track_audio', { stream: this.stream }).catch(() => {});
    this.stream = 0;
    this.ready = false;
    this.playing = false;
    this.done = false;
    this.frames = 0;
    this.frame = 0;
    this.durationMs = 0;
    this.node?.port.postMessage({ type: 'start', frames: 0, step: 1, seq: ++this.seq });
    this.rest();
  }

  /** The audio device stays open while paused, so play is instant; left paused, it's let go. */
  private rest() {
    clearTimeout(this.idle);
    this.idle = window.setTimeout(() => {
      if (!this.playing) void this.context?.suspend();
    }, IDLE_MS);
  }

  dispose() {
    this.unload();
    clearTimeout(this.idle);
    this.onstate = () => {};
    this.onended = () => {};
    this.onerror = () => {};
    void this.context?.close();
    this.context = null;
    this.node = null;
  }
}
