/**
 * Framebuffer Worker — renders screen frames on OffscreenCanvas
 *
 * Receives frames from the main thread (via messages or a MessagePort),
 * decodes image data, and draws to canvases transferred from the main thread.
 * Supports stale frame protection via sequence numbers.
 */

/** @type {Map<string, { canvas: OffscreenCanvas, ctx: OffscreenCanvasRenderingContext2D, paintedSeq: number }>} */
const canvases = new Map();

/** @type {Map<string, string>} sessionId -> canvasId */
const bindings = new Map();

/** @type {MessagePort|null} */
let port = null;

/**
 * Handle incoming messages from main thread or port.
 * @param {MessageEvent} e
 */
function handleMessage(e) {
  const msg = e.data;

  switch (msg.kind) {
    case 'port': {
      // Receive a MessagePort for communication
      port = msg.port;
      port.onmessage = handleMessage;
      break;
    }

    case 'adopt': {
      // Receive an OffscreenCanvas transferred from main thread
      const { canvasId, canvas } = msg;
      const ctx = canvas.getContext('2d');
      if (!ctx) {
        console.error('[FramebufferWorker] Failed to get 2d context for canvas:', canvasId);
        return;
      }
      canvases.set(canvasId, { canvas, ctx, paintedSeq: -1 });
      break;
    }

    case 'bind': {
      // Bind a session to a canvas
      const { sessionId, canvasId } = msg;
      bindings.set(sessionId, canvasId);
      break;
    }

    case 'unbind': {
      // Unbind a session
      const { sessionId } = msg;
      bindings.delete(sessionId);
      break;
    }

    case 'release': {
      // Release a canvas (session ended or component destroyed)
      const { canvasId } = msg;
      canvases.delete(canvasId);
      // Also remove any bindings pointing to this canvas
      for (const [sid, cid] of bindings.entries()) {
        if (cid === canvasId) {
          bindings.delete(sid);
        }
      }
      break;
    }

    case 'frame': {
      // Render a frame
      renderFrame(msg);
      break;
    }

    default:
      console.warn('[FramebufferWorker] Unknown message kind:', msg.kind);
  }
}

/**
 * Decode and render a frame to the bound canvas.
 * @param {{ sessionId: string, seq: number, width: number, height: number, mimeType: string, data: Uint8Array }} frame
 */
async function renderFrame(frame) {
  const { sessionId, seq, width, height, mimeType, data } = frame;

  const canvasId = bindings.get(sessionId);
  if (!canvasId) return;

  const entry = canvases.get(canvasId);
  if (!entry) return;

  // Drop stale frames (out-of-order or duplicate)
  if (seq <= entry.paintedSeq) return;

  const { canvas, ctx } = entry;

  // Resize canvas if dimensions changed
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }

  try {
    const bitmap = await decodeFrame(data, mimeType);
    ctx.drawImage(bitmap, 0, 0, width, height);
    bitmap.close();
    entry.paintedSeq = seq;
  } catch (err) {
    console.error('[FramebufferWorker] Frame decode/draw error:', err);
  }
}

/**
 * Decode frame data to an ImageBitmap.
 * Uses ImageDecoder API if available (Chrome/Edge), falls back to createImageBitmap.
 * @param {Uint8Array} data
 * @param {string} mimeType
 * @returns {Promise<ImageBitmap>}
 */
async function decodeFrame(data, mimeType) {
  // Try ImageDecoder API (better performance, available in Chromium-based browsers)
  if (typeof ImageDecoder !== 'undefined') {
    try {
      const decoder = new ImageDecoder({
        type: mimeType || 'image/png',
        data: data
      });
      const result = await decoder.decode();
      const bitmap = result.image;
      decoder.close();
      return bitmap;
    } catch {
      // Fall through to createImageBitmap
    }
  }

  // Fallback: createImageBitmap from Blob
  const blob = new Blob([data], { type: mimeType || 'image/png' });
  return createImageBitmap(blob);
}

self.onmessage = handleMessage;
