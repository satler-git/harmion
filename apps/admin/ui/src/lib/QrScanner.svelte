<script lang="ts">
  import { onDestroy } from 'svelte';
  import { errorMessage } from './api.svelte';
  let { onread }: { onread: (text: string) => void } = $props();
  let video: HTMLVideoElement;
  let error = $state('');
  let scanning = $state(false);
  let stream: MediaStream | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let sequence = 0;

  function stop() {
    sequence++;
    if (timer) clearTimeout(timer);
    timer = undefined;
    stream?.getTracks().forEach((track) => track.stop());
    stream = undefined;
    if (video) video.srcObject = null;
    scanning = false;
  }

  function cameraError(cause: unknown) {
    if (cause instanceof DOMException) {
      switch (cause.name) {
        case 'NotAllowedError':
        case 'SecurityError':
          return 'カメラの使用が許可されていません。ブラウザのサイト設定で許可するか、QR 画像を選択してください。';
        case 'NotFoundError':
        case 'OverconstrainedError':
          return '利用できるカメラが見つかりません。QR 画像を選択するか、接続コードを貼り付けてください。';
        case 'NotReadableError':
          return 'カメラを開始できませんでした。他のアプリが使用していないか確認してください。';
      }
    }
    return errorMessage(cause);
  }

  async function openCamera(): Promise<MediaStream> {
    if (!window.isSecureContext) {
      throw new Error(
        'カメラは HTTPS の管理画面でのみ利用できます。管理画面の証明書を信頼するか、QR 画像を選択してください。',
      );
    }
    const media = navigator.mediaDevices;
    if (!media?.getUserMedia) {
      throw new Error(
        'このブラウザではカメラを利用できません。QR 画像を選択するか、接続コードを貼り付けてください。',
      );
    }
    const size = { width: { ideal: 1920 }, height: { ideal: 1080 } };
    try {
      return await media.getUserMedia({
        video: { ...size, facingMode: { ideal: 'environment' } },
        audio: false,
      });
    } catch (cause) {
      if (
        cause instanceof DOMException &&
        (cause.name === 'NotAllowedError' || cause.name === 'SecurityError')
      ) {
        throw cause;
      }
      // A front-only or fixed-focus camera may reject the environment hint or
      // the preferred resolution. Ask for the defaults before giving up.
      return await media.getUserMedia({ video: size, audio: false });
    }
  }

  async function start() {
    const attempt = ++sequence;
    error = '';
    scanning = true;
    try {
      const { default: jsQR } = await import('jsqr');
      if (attempt !== sequence) return;
      const camera = await openCamera();
      if (attempt !== sequence) {
        camera.getTracks().forEach((track) => track.stop());
        return;
      }
      stream = camera;
      const track = camera.getVideoTracks()[0];
      if (track) {
        // Continuous autofocus helps with a phone screen held close to a
        // webcam. Ignore browsers and cameras that do not expose the control.
        try {
          await track.applyConstraints({
            advanced: [{ focusMode: 'continuous' } as MediaTrackConstraintSet],
          });
        } catch {
          // Not supported; the default focus mode is still usable.
        }
      }
      video.srcObject = camera;
      await video.play();
      const canvas = document.createElement('canvas');
      const context = canvas.getContext('2d', { willReadFrequently: true })!;
      function frame() {
        if (attempt !== sequence) return;
        try {
          if (video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA && video.videoWidth > 0) {
            const scale = Math.min(1, 1600 / Math.max(video.videoWidth, video.videoHeight));
            canvas.width = Math.round(video.videoWidth * scale);
            canvas.height = Math.round(video.videoHeight * scale);
            context.drawImage(video, 0, 0, canvas.width, canvas.height);
            const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
            const result = jsQR(pixels.data, pixels.width, pixels.height);
            if (result) {
              stop();
              onread(result.data);
              return;
            }
          }
        } catch (cause) {
          if (attempt === sequence) {
            stop();
            error = cameraError(cause);
          }
          return;
        }
        timer = setTimeout(frame, 150);
      }
      frame();
    } catch (cause) {
      if (attempt === sequence) {
        stop();
        error = cameraError(cause);
      }
    }
  }

  async function image(event: Event) {
    stop();
    error = '';
    const attempt = sequence;
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    // Allow choosing the same file again after a failed attempt.
    input.value = '';
    if (!file) return;
    const url = URL.createObjectURL(file);
    try {
      const { default: jsQR } = await import('jsqr');
      const source = new Image();
      source.src = url;
      await source.decode();
      if (attempt !== sequence) return;
      const canvas = document.createElement('canvas');
      const scale = Math.min(1, 2048 / Math.max(source.naturalWidth, source.naturalHeight));
      canvas.width = Math.round(source.naturalWidth * scale);
      canvas.height = Math.round(source.naturalHeight * scale);
      const context = canvas.getContext('2d', { willReadFrequently: true })!;
      context.drawImage(source, 0, 0, canvas.width, canvas.height);
      const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
      const result = jsQR(pixels.data, pixels.width, pixels.height);
      if (!result) throw new Error('QR code not found');
      if (attempt === sequence) onread(result.data);
    } catch {
      if (attempt === sequence)
        error =
          '画像から QR を読み取れませんでした。ピントと余白を確認するか接続コードを貼り付けてください。';
    } finally {
      URL.revokeObjectURL(url);
    }
  }
  onDestroy(stop);
</script>

<div class="scanner">
  <div class="actions">
    <button onclick={scanning ? stop : start}>
      {scanning ? 'カメラを停止' : 'カメラで QR を読み取る'}
    </button>
    <label class="file-button">
      QR 画像を選択<input type="file" accept="image/*" onchange={image} />
    </label>
  </div>
  <video
    bind:this={video}
    class:hidden={!scanning}
    muted
    autoplay
    playsinline
    aria-label="Peerの QR スキャナー"
  ></video>
  {#if scanning}
    <p class="muted small" role="status">
      QR コードをカメラの中央に収めてください。読み取ると自動で停止します。
    </p>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</div>
