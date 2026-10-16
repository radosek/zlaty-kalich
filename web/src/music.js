// MIDI music for the browser, replacing the original player.exe helper:
// each track loops until the next command, as the TMediaPlayer did.
// Bundled with esbuild into dist/lib/music.js.

import { Sequencer, WorkletSynthesizer } from "spessasynth_lib";

export class Music {
  #ctx;
  #gain;
  #seq;
  #ready;
  #current = null;

  /** @param {string} processorUrl @param {() => Promise<ArrayBuffer>} loadSoundBank */
  constructor(processorUrl, loadSoundBank, volume = 0.8) {
    this.#ctx = new AudioContext();
    this.#gain = this.#ctx.createGain();
    this.#gain.gain.value = volume;
    this.#gain.connect(this.#ctx.destination);
    this.#ready = (async () => {
      await this.#ctx.audioWorklet.addModule(processorUrl);
      const synth = new WorkletSynthesizer(this.#ctx);
      synth.connect(this.#gain);
      await synth.isReady;
      await synth.soundBankManager.addSoundBank(await loadSoundBank(), "main");
      this.#seq = new Sequencer(synth);
      this.#seq.loopCount = -1;
    })();
  }

  /** Must run from a user gesture once, so the browser lets audio start. */
  resume() {
    return this.#ctx.resume();
  }

  /** @param {string} name @param {ArrayBuffer} midi */
  async play(name, midi) {
    if (this.#current === name) return;
    this.#current = name;
    await this.#ready;
    this.#seq.loadNewSongList([{ binary: midi, fileName: name }]);
    this.#seq.loopCount = -1;
    this.#seq.play();
  }

  async stop() {
    this.#current = null;
    await this.#ready;
    this.#seq.pause();
  }

  setVolume(volume) {
    this.#gain.gain.value = volume;
  }
}
