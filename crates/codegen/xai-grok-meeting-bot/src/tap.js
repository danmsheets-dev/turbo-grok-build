// Turbo meeting tap. Installed via Page.addScriptToEvaluateOnNewDocument, so
// this runs before any Teams script in every frame.
//
// Three jobs:
//   1. Wrap RTCPeerConnection, collect inbound audio, stream 16 kHz mono
//      16-bit LE PCM to Turbo over a loopback WebSocket.
//   2. Replace getUserMedia's audio with a silent synthetic track, so the bot
//      never emits Chromium's fake-device beep into a real meeting. The same
//      node is where v2 will push TTS.
//   3. Scrape chat and roster from the DOM and report them over a CDP binding.
//
// Everything is best-effort and must never throw into Teams' own code.
(() => {
  'use strict';

  const CFG = globalThis.__TURBO_CFG;
  if (!CFG || globalThis.__turboTapInstalled) return;
  globalThis.__turboTapInstalled = true;

  const SEL = CFG.selectors || {};
  const isTopFrame = (() => {
    try {
      return window.self === window.top;
    } catch {
      return false;
    }
  })();

  // ---------------------------------------------------------------- reporting

  function report(obj) {
    try {
      const fn = globalThis[CFG.bindingName];
      if (typeof fn === 'function') fn(JSON.stringify(obj));
    } catch {
      /* binding not attached yet; drop */
    }
  }

  function reportError(step, err) {
    report({ type: 'error', step, message: String((err && err.message) || err) });
  }

  // -------------------------------------------------------------- audio egress

  // Silent outbound track. Teams needs *an* audio track to complete the join;
  // giving it a muted synthetic one avoids both the fake-device beep and any
  // chance of capturing the operator's real microphone.
  let outboundCtx = null;
  let outboundDest = null;

  function silentOutboundTrack() {
    try {
      if (!outboundDest) {
        outboundCtx = new (globalThis.AudioContext || globalThis.webkitAudioContext)();
        outboundDest = outboundCtx.createMediaStreamDestination();
        // A stopped oscillator still yields a live-but-silent track.
        const osc = outboundCtx.createOscillator();
        const gain = outboundCtx.createGain();
        gain.gain.value = 0;
        osc.connect(gain).connect(outboundDest);
        osc.start();
        globalThis.__turboSpeechDestination = outboundDest;
        globalThis.__turboSpeechContext = outboundCtx;
      }
      return outboundDest.stream.getAudioTracks()[0] || null;
    } catch (e) {
      reportError('silent-outbound', e);
      return null;
    }
  }

  const mediaDevices = navigator.mediaDevices;
  if (mediaDevices && typeof mediaDevices.getUserMedia === 'function') {
    const original = mediaDevices.getUserMedia.bind(mediaDevices);
    mediaDevices.getUserMedia = async (constraints) => {
      const want = constraints || {};
      // Video is refused outright: a notetaker has no camera.
      if (want.audio && !want.video) {
        const track = silentOutboundTrack();
        if (track) return new MediaStream([track]);
      }
      if (want.video) {
        const track = want.audio ? silentOutboundTrack() : null;
        return new MediaStream(track ? [track] : []);
      }
      return original(constraints);
    };
  }

  // ------------------------------------------------------------- audio ingress

  const WORKLET_SRC = `
    class TurboTap extends AudioWorkletProcessor {
      constructor() {
        super();
        this.buf = new Int16Array(${CFG.frameSamples});
        this.n = 0;
      }
      process(inputs) {
        const ch = inputs[0] && inputs[0][0];
        if (ch) {
          for (let i = 0; i < ch.length; i++) {
            let s = ch[i];
            if (s > 1) s = 1; else if (s < -1) s = -1;
            this.buf[this.n++] = s < 0 ? s * 0x8000 : s * 0x7fff;
            if (this.n === this.buf.length) {
              this.port.postMessage(this.buf.slice());
              this.n = 0;
            }
          }
        }
        return true;
      }
    }
    registerProcessor('turbo-tap', TurboTap);
  `;

  let socket = null;
  let inCtx = null;
  let mixer = null;
  let graphReady = null;
  let dropped = 0;
  // Weak: a long meeting churns tracks as people join and leave, and a strong
  // set would pin every one of them for the whole call. `WeakSet` supports the
  // `delete` used on the graph-failure retry path.
  const attached = new WeakSet();

  let lastSocketAttempt = 0;

  function openSocket() {
    if (socket) return socket;
    // Throttle: a refused connect must not turn into a reconnect storm at the
    // worklet's message rate (50/s).
    const now = Date.now();
    if (now - lastSocketAttempt < 2000) return null;
    lastSocketAttempt = now;
    try {
      socket = new WebSocket(CFG.audioUrl);
      socket.binaryType = 'arraybuffer';
      socket.onclose = () => {
        socket = null;
      };
      socket.onerror = () => {
        report({ type: 'audio', state: 'socket-error' });
      };
      socket.onopen = () => report({ type: 'audio', state: 'streaming' });
    } catch (e) {
      reportError('audio-socket', e);
      socket = null;
    }
    return socket;
  }

  // A memoized promise, not a boolean. Participants join at the same moment,
  // so several `track` events land while `audioWorklet.addModule` is still
  // awaiting. A flag latched before that await would let every racer past
  // while `mixer` was still undefined, and each of those tracks is already in
  // the `attached` WeakSet — so they would be dropped for the whole meeting.
  function startAudioGraph() {
    if (graphReady) return graphReady;
    graphReady = (async () => {
      // Running the graph natively at the STT rate means no manual resampling:
      // remote tracks are converted into the context rate for us.
      inCtx = new (globalThis.AudioContext || globalThis.webkitAudioContext)({
        sampleRate: CFG.sampleRate,
      });
      const blob = new Blob([WORKLET_SRC], { type: 'application/javascript' });
      const url = URL.createObjectURL(blob);
      await inCtx.audioWorklet.addModule(url);
      URL.revokeObjectURL(url);

      mixer = inCtx.createGain();
      // The worklet reads inputs[0][0], so force a mono downmix here. Without
      // this an explicit-stereo remote track would have its right channel
      // silently discarded rather than summed.
      mixer.channelCount = 1;
      mixer.channelCountMode = 'explicit';
      mixer.channelInterpretation = 'speakers';
      const node = new AudioWorkletNode(inCtx, 'turbo-tap', {
        channelCount: 1,
        channelCountMode: 'explicit',
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [1],
      });
      node.port.onmessage = (ev) => {
        if (!socket) {
          // The sink went away mid-meeting. Reopen (throttled) instead of
          // going permanently silent — audio is the whole point of the bot.
          openSocket();
          return;
        }
        const ws = socket.readyState === WebSocket.OPEN ? socket : null;
        if (!ws) return;
        // Shed rather than queue. If Turbo stops reading (an STT reconnect),
        // buffering live meeting audio in the renderer only grows a backlog
        // that is stale by the time it is transcribed.
        if (ws.bufferedAmount > CFG.maxBufferedBytes) {
          dropped += 1;
          if (dropped % 50 === 1) report({ type: 'audio', state: 'dropping' });
          return;
        }
        ws.send(ev.data.buffer);
      };
      mixer.connect(node);
      // A worklet only runs while the graph is pulled. Route to the speakers
      // through a zero gain so nothing is actually played back.
      const mute = inCtx.createGain();
      mute.gain.value = 0;
      node.connect(mute).connect(inCtx.destination);

      if (inCtx.state === 'suspended') await inCtx.resume();
      openSocket();
    })().catch((e) => {
      // Clear so a later track can retry a failed build.
      graphReady = null;
      inCtx = null;
      mixer = null;
      reportError('audio-graph', e);
    });
    return graphReady;
  }

  async function attachTrack(track) {
    if (!track || track.kind !== 'audio' || attached.has(track)) return;
    attached.add(track);
    await startAudioGraph();
    if (!inCtx || !mixer) {
      // The graph failed to build; let a later track try again.
      attached.delete(track);
      return;
    }
    try {
      const src = inCtx.createMediaStreamSource(new MediaStream([track]));
      src.connect(mixer);
      track.addEventListener('ended', () => {
        try {
          src.disconnect();
        } catch {
          /* already torn down */
        }
      });
      report({ type: 'audio', state: 'track', id: track.id });
    } catch (e) {
      reportError('attach-track', e);
    }
  }

  const NativePC = globalThis.RTCPeerConnection;
  if (NativePC) {
    const Wrapped = function (...args) {
      const pc = new NativePC(...args);
      pc.addEventListener('track', (ev) => {
        if (ev.track && ev.track.kind === 'audio') void attachTrack(ev.track);
        if (ev.streams) {
          for (const s of ev.streams) for (const t of s.getAudioTracks()) void attachTrack(t);
        }
      });
      return pc;
    };
    Wrapped.prototype = NativePC.prototype;
    for (const k of Object.keys(NativePC)) {
      try {
        Wrapped[k] = NativePC[k];
      } catch {
        /* non-writable static */
      }
    }
    globalThis.RTCPeerConnection = Wrapped;
    globalThis.webkitRTCPeerConnection = Wrapped;
  }

  // ------------------------------------------------------------------ DOM side

  function q(list) {
    for (const sel of list || []) {
      try {
        const el = document.querySelector(sel);
        if (el) return el;
      } catch {
        /* malformed selector override */
      }
    }
    return null;
  }

  function qa(list) {
    for (const sel of list || []) {
      try {
        const els = document.querySelectorAll(sel);
        if (els.length) return Array.from(els);
      } catch {
        /* malformed selector override */
      }
    }
    return [];
  }

  function textOf(el) {
    return ((el && (el.innerText || el.textContent)) || '').trim();
  }

  function pageHasText(needles) {
    const body = (document.body && document.body.innerText) || '';
    const hay = body.toLowerCase();
    return (needles || []).some((n) => hay.includes(String(n).toLowerCase()));
  }

  // Join state is reported, never forced. Teams decides lobby vs admitted.
  function detectState() {
    if (q(SEL.callControls)) return 'admitted';
    if (q(SEL.lobbyIndicator) || pageHasText(SEL.lobbyText)) return 'lobby';
    if (pageHasText(SEL.deniedText)) return 'denied';
    if (pageHasText(SEL.captchaText)) return 'captcha';
    if (pageHasText(SEL.signInRequiredText)) return 'sign-in-required';
    if (q(SEL.nameInput) || q(SEL.joinButton)) return 'prejoin';
    return 'loading';
  }

  const seenChat = new Set();
  // A long meeting must not grow this without bound. Teams only keeps a window
  // of messages mounted anyway, so an old key can be forgotten safely.
  const SEEN_CHAT_MAX = 2000;

  // Prefer Teams' own message id. Falling back to position would re-report the
  // whole backlog every time the virtualized list re-indexes on scroll.
  function chatKey(node, from, text) {
    const id =
      node.getAttribute('data-mid') ||
      node.getAttribute('data-item-id') ||
      node.id ||
      '';
    return id ? `id:${id}` : JSON.stringify([from, text]);
  }

  function remember(key) {
    if (seenChat.size >= SEEN_CHAT_MAX) {
      // Drop the oldest insertion; Set iteration order is insertion order.
      const oldest = seenChat.values().next();
      if (!oldest.done) seenChat.delete(oldest.value);
    }
    seenChat.add(key);
  }

  function scrapeChat() {
    for (const node of qa(SEL.chatMessage)) {
      const from = textOf(q0(node, SEL.chatAuthor)) || 'meeting';
      const body = textOf(q0(node, SEL.chatBody) || node);
      if (!body) continue;
      const key = chatKey(node, from, body);
      if (seenChat.has(key)) continue;
      remember(key);
      // Never interpret; hand the raw text to Turbo as data.
      report({ type: 'chat', from, text: body });
    }
  }

  function q0(root, list) {
    for (const sel of list || []) {
      try {
        const el = root.querySelector(sel);
        if (el) return el;
      } catch {
        /* malformed selector override */
      }
    }
    return null;
  }

  function scrapeRoster() {
    const names = qa(SEL.participant)
      .map((el) => textOf(el))
      .filter(Boolean);
    if (names.length) report({ type: 'roster', names });
  }

  let lastState = '';
  function poll() {
    try {
      const state = detectState();
      if (state !== lastState) {
        lastState = state;
        report({ type: 'state', state });
      }
      if (state === 'admitted') {
        scrapeChat();
        scrapeRoster();
      }
    } catch (e) {
      reportError('poll', e);
    }
  }

  if (isTopFrame) {
    const startPolling = () => {
      poll();
      setInterval(poll, CFG.pollMs);
    };
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', startPolling, { once: true });
    } else {
      startPolling();
    }
  }

  // Exposed for Rust-driven steps (fill name, click join, post chat).
  globalThis.__turbo = {
    state: () => detectState(),

    // Teams offers an app-download interstitial before pre-join.
    continueInBrowser() {
      const el = q(SEL.continueInBrowser);
      if (!el) return false;
      el.click();
      return true;
    },

    setName(name) {
      const el = q(SEL.nameInput);
      if (!el) return false;
      const setter = Object.getOwnPropertyDescriptor(
        globalThis.HTMLInputElement.prototype,
        'value',
      ).set;
      setter.call(el, name);
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    },

    // Mute mic and camera before joining. Belt and braces: getUserMedia is
    // already neutered, but the UI state is what other participants see.
    muteDevices() {
      let acted = false;
      for (const list of [SEL.micToggle, SEL.cameraToggle]) {
        const el = q(list);
        if (!el) continue;
        const pressed = el.getAttribute('aria-pressed') === 'true';
        const checked = el.getAttribute('aria-checked') === 'true';
        if (pressed || checked) {
          el.click();
          acted = true;
        }
      }
      return acted;
    },

    clickJoin() {
      const el = q(SEL.joinButton);
      if (!el || el.disabled) return false;
      el.click();
      return true;
    },

    participants() {
      return qa(SEL.participant).map((el) => textOf(el)).filter(Boolean);
    },

    postChat(text) {
      const box = q(SEL.chatInput);
      if (!box) return false;
      box.focus();
      // Teams' composer is a contenteditable; insertText keeps its model in sync.
      const ok = document.execCommand('insertText', false, text);
      if (!ok) {
        box.textContent = text;
        box.dispatchEvent(new InputEvent('input', { bubbles: true, data: text }));
      }
      const send = q(SEL.chatSend);
      if (send && !send.disabled) {
        send.click();
        return true;
      }
      box.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', keyCode: 13, bubbles: true }),
      );
      return true;
    },
  };
})();
