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

  async function start() {
    const attempt = ++sequence;
    error = '';
    scanning = true;
    try {
      const { default: jsQR } = await import('jsqr');
      if (attempt !== sequence) return;
      const camera = await navigator.mediaDevices.getUserMedia({
        video: { facingMode: 'environment', width: { ideal: 1600 } },
        audio: false,
      });
      if (attempt !== sequence) {
        camera.getTracks().forEach((track) => track.stop());
        return;
      }
      stream = camera;
      video.srcObject = camera;
      await video.play();
      const canvas = document.createElement('canvas');
      const context = canvas.getContext('2d', { willReadFrequently: true })!;
      function frame() {
        if (attempt !== sequence) return;
        if (video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA) {
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
        timer = setTimeout(frame, 200);
      }
      frame();
    } catch (cause) {
      if (attempt === sequence) {
        stop();
        error = errorMessage(cause);
      }
    }
  }

  async function image(event: Event) {
    stop();
    error = '';
    const attempt = sequence;
    const file = (event.currentTarget as HTMLInputElement).files?.[0];
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
    playsinline
    aria-label="Peer の QR スキャナー"
  ></video>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</div>
