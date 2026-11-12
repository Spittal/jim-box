// The player's audio thread: holds a track's samples as they are decoded and
// plays them. Plain JavaScript, loaded as text by trackAudio.ts, which also
// describes the messages. Starting, stopping and seeking are a message each,
// acted on within one block of 128 samples: about three milliseconds.

/** Blocks between reports of where playing has got to: about 12 ms. */
const REPORT_EVERY = 4;

class TrackPlayer extends AudioWorkletProcessor {
  constructor() {
    super();
    /** 16-bit stereo samples, left then right. */
    this.pcm = new Int16Array(0);
    /** Frames of `pcm` decoded so far. */
    this.filled = 0;
    /** No more are coming. */
    this.done = false;
    /** The frame playing is at; a fraction when the rates differ. */
    this.at = 0;
    /** Frames of the track to each one put out. */
    this.step = 1;
    /** How fast it plays, 1 being as recorded: pitch follows, as on a turntable. */
    this.speed = 1;
    /** The frames a loop goes round between; no loop while the end isn't after the start. */
    this.loopStart = 0;
    this.loopEnd = 0;
    this.playing = false;
    /** The main thread's count of loads and seeks, sent back with reports. */
    this.seq = 0;
    this.blocks = 0;
    this.port.onmessage = (event) => this.receive(event.data);
  }

  receive(message) {
    switch (message.type) {
      case 'start':
        this.pcm = new Int16Array(message.frames * 2);
        this.filled = 0;
        this.done = false;
        this.at = 0;
        this.step = message.step;
        this.speed = message.speed ?? 1;
        this.loopStart = 0;
        this.loopEnd = 0;
        this.playing = false;
        this.seq = message.seq;
        break;
      case 'chunk': {
        const samples = new Int16Array(message.data);
        const needed = this.filled * 2 + samples.length;
        // More than was said to be coming: make room.
        if (needed > this.pcm.length) {
          const grown = new Int16Array(Math.max(needed, Math.ceil(this.pcm.length * 1.5)));
          grown.set(this.pcm.subarray(0, this.filled * 2));
          this.pcm = grown;
        }
        this.pcm.set(samples, this.filled * 2);
        this.filled += samples.length / 2;
        break;
      }
      case 'done':
        this.done = true;
        break;
      case 'play':
        this.playing = true;
        this.report();
        break;
      case 'pause':
        this.playing = false;
        this.report();
        break;
      case 'seek':
        this.at = Math.max(0, message.frame);
        this.seq = message.seq;
        this.report();
        break;
      case 'speed':
        this.speed = message.speed;
        this.report();
        break;
      case 'loop':
        this.loopStart = message.start;
        this.loopEnd = message.end;
        break;
    }
  }

  report() {
    this.port.postMessage({ type: 'at', frame: this.at, time: currentTime, seq: this.seq });
  }

  process(_inputs, outputs) {
    if (!this.playing) return true;
    if (this.blocks++ % REPORT_EVERY === 0) this.report();
    const left = outputs[0][0];
    const right = outputs[0][1] ?? left;
    const { pcm, loopStart, loopEnd } = this;
    const step = this.step * this.speed;
    const looping = loopEnd > loopStart;
    const last = this.filled - 1;
    let at = this.at;
    let i = 0;
    for (; i < left.length; i++) {
      // Round the loop to the sample, so it stays in time however short it is.
      if (looping && at >= loopEnd) at = loopStart + ((at - loopEnd) % (loopEnd - loopStart));
      const frame = Math.floor(at);
      // The frame after is needed too, to go between them.
      if (frame >= last) break;
      const part = at - frame;
      const from = frame * 2;
      left[i] = (pcm[from] + (pcm[from + 2] - pcm[from]) * part) / 32768;
      right[i] = (pcm[from + 1] + (pcm[from + 3] - pcm[from + 1]) * part) / 32768;
      at += step;
    }
    this.at = at;
    // Out of samples: the end of the track, or the decoding hasn't got this
    // far yet, and playing carries on when it has.
    if (i < left.length && this.done) {
      this.playing = false;
      this.port.postMessage({ type: 'ended', seq: this.seq });
    }
    return true;
  }
}

registerProcessor('track-player', TrackPlayer);
