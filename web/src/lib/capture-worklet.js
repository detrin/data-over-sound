// Batches 128-sample render quanta into larger chunks and forwards them to the
// audio worker over a MessagePort, so the main thread never touches samples.
const CHUNK = 1024;

class Capture extends AudioWorkletProcessor {
  constructor() {
    super();
    this.port_ = null;
    this.buf = new Float32Array(CHUNK);
    this.n = 0;
    this.port.onmessage = (e) => {
      if (e.data && e.data.port) this.port_ = e.data.port;
    };
  }

  process(inputs) {
    const ch = inputs[0] && inputs[0][0];
    if (ch && this.port_) {
      let i = 0;
      while (i < ch.length) {
        const take = Math.min(ch.length - i, CHUNK - this.n);
        this.buf.set(ch.subarray(i, i + take), this.n);
        this.n += take;
        i += take;
        if (this.n === CHUNK) {
          this.port_.postMessage(this.buf, [this.buf.buffer]);
          this.buf = new Float32Array(CHUNK);
          this.n = 0;
        }
      }
    }
    return true;
  }
}

registerProcessor('dos-capture', Capture);
