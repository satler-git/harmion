package org.harmion.mote.voice

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

internal const val PROBE_SAMPLE_RATE = 16_000
private const val CHUNK_SAMPLES = PROBE_SAMPLE_RATE * 5
private const val MAX_RECORDING_MS = 60 * 60 * 1_000L
private const val MAX_RECORDING_SAMPLES = 60 * 60 * PROBE_SAMPLE_RATE
private const val MAX_STORAGE_BYTES = 256L * 1024 * 1024
private const val WAV_HEADER_BYTES = 44

internal data class VoiceProbeState(
    val recordingId: String? = null,
    val message: String = "停止中",
    val savedSamples: Int = 0,
    val active: Boolean = false,
    val stopping: Boolean = false,
)

class VoiceProbeService : Service() {
    private class Recording(val id: String = UUID.randomUUID().toString()) {
        val stop = AtomicBoolean()
    }

    private val main = Handler(Looper.getMainLooper())
    private var current: Recording? = null

    override fun onBind(intent: Intent?) = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            START -> if (current == null) begin()
            STOP -> {
                val recording = current
                if (recording != null && intent.data?.lastPathSegment == recording.id) {
                    recording.stop.set(true)
                    status.value = status.value.copy(message = "停止処理中", stopping = true)
                } else if (recording == null) {
                    stopSelf()
                }
            }
            else -> if (current == null) stopSelf()
        }
        return START_NOT_STICKY
    }

    private fun begin() {
        val notifications = getSystemService(NotificationManager::class.java)
        val recording = Recording()
        try {
            startForeground(NOTIFICATION_ID, notification(recording), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
        } catch (_: SecurityException) {
            status.value = VoiceProbeState(message = "マイクを使用できませんでした。権限を確認してください。")
            stopSelf()
            return
        } catch (_: IllegalStateException) {
            status.value = VoiceProbeState(message = "画面を開いたまま録音を開始してください。")
            stopSelf()
            return
        }
        if (!notificationsAvailable(notifications)) {
            status.value = VoiceProbeState(message = "録音検証の通知を有効にしてください。")
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return
        }

        current = recording
        status.value = VoiceProbeState(recording.id, message = "録音を準備中", active = true)
        Thread({ record(recording) }, "Mote voice probe").start()
    }

    private fun notification(recording: Recording): Notification {
        val stop = PendingIntent.getService(
            this, 0, stopIntent(this, recording.id), PendingIntent.FLAG_IMMUTABLE,
        )
        val open = PendingIntent.getActivity(
            this, 0,
            Intent(this, VoiceProbeActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle("Mote 録音検証")
            .setContentText("マイクを使用しています")
            .setContentIntent(open)
            .setOngoing(true)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .addAction(android.R.drawable.ic_media_pause, "停止", stop)
            .build()
    }

    // The Activity obtains RECORD_AUDIO before starting the service. Android also checks
    // the grant when the service promotes itself. A later revocation can still abort input.
    @SuppressLint("MissingPermission")
    private fun record(recording: Recording) {
        var savedSamples = 0
        var message = "録音を停止しました。"
        val wake = getSystemService(PowerManager::class.java)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Mote:VoiceProbe")
        try {
            if (recording.stop.get()) return
            val directory = File(noBackupFilesDir, "voice-probe")
            if (!directory.isDirectory && !directory.mkdirs()) throw IOException("Cannot create recording directory")
            val existing = directory.listFiles() ?: throw IOException("Cannot list recording directory")
            var remainingBytes = MAX_STORAGE_BYTES - existing.sumOf { it.length() }
            if (remainingBytes < WAV_HEADER_BYTES + CHUNK_SAMPLES * Short.SIZE_BYTES) throw IOException("Recording quota")

            val minBuffer = AudioRecord.getMinBufferSize(
                PROBE_SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
            )
            check(minBuffer > 0)
            val block = ShortArray(PROBE_SAMPLE_RATE / 10)
            val pcm = ByteBuffer.allocate(CHUNK_SAMPLES * Short.SIZE_BYTES).order(ByteOrder.LITTLE_ENDIAN)
            val samples = pcm.asShortBuffer()
            val audio = AudioRecord.Builder()
                .setAudioSource(MediaRecorder.AudioSource.MIC)
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .setSampleRate(PROBE_SAMPLE_RATE)
                        .setChannelMask(AudioFormat.CHANNEL_IN_MONO)
                        .build(),
                )
                .setBufferSizeInBytes(maxOf(minBuffer, block.size * Short.SIZE_BYTES * 4))
                .build()
            try {
                check(audio.state == AudioRecord.STATE_INITIALIZED)
                if (recording.stop.get()) return
                wake.acquire(MAX_RECORDING_MS + 60_000L)
                audio.startRecording()
                check(audio.recordingState == AudioRecord.RECORDSTATE_RECORDING)
                publish(recording) { if (stopping) this else copy(message = "録音中") }
                val deadline = SystemClock.elapsedRealtime() + MAX_RECORDING_MS
                while (!recording.stop.get() && savedSamples < MAX_RECORDING_SAMPLES &&
                    SystemClock.elapsedRealtime() < deadline
                ) {
                    val count = audio.read(block, 0, minOf(block.size, samples.remaining()), AudioRecord.READ_BLOCKING)
                    if (count <= 0) {
                        message = "マイクからの読み取りが途切れました。"
                        break
                    }
                    samples.put(block, 0, count)
                    if (!samples.hasRemaining()) {
                        remainingBytes = writeChunk(directory, recording.id, savedSamples, pcm.array(), samples.position(), remainingBytes)
                        savedSamples += samples.position()
                        samples.clear()
                        val saved = savedSamples
                        publish(recording) { copy(savedSamples = saved) }
                    }
                }
                audio.stop()
                if (samples.position() > 0) {
                    writeChunk(directory, recording.id, savedSamples, pcm.array(), samples.position(), remainingBytes)
                    savedSamples += samples.position()
                }
                if (savedSamples >= MAX_RECORDING_SAMPLES || SystemClock.elapsedRealtime() >= deadline) {
                    message = "検証用の 60 分上限で停止しました。"
                }
            } finally {
                audio.release()
            }
        } catch (_: IOException) {
            message = "保存できませんでした。検証用の容量上限または空き容量を確認してください。"
        } catch (_: SecurityException) {
            message = "マイクの使用が許可されていません。"
        } catch (_: IllegalArgumentException) {
            message = "この端末で検証用の録音形式を開けませんでした。"
        } catch (_: IllegalStateException) {
            message = "マイクを使用できませんでした。"
        } finally {
            if (wake.isHeld) wake.release()
            main.post {
                if (current === recording) {
                    current = null
                    status.value = VoiceProbeState(recording.id, message, savedSamples)
                    stopForeground(STOP_FOREGROUND_REMOVE)
                    stopSelf()
                }
            }
        }
    }

    private fun publish(recording: Recording, update: VoiceProbeState.() -> VoiceProbeState) {
        main.post { if (current === recording) status.value = status.value.update() }
    }

    override fun onDestroy() {
        current?.let {
            it.stop.set(true)
            status.value = status.value.copy(active = false, stopping = false, message = "録音サービスが終了しました。")
        }
        current = null
        super.onDestroy()
    }

    internal companion object {
        private const val START = "org.harmion.mote.voice.START_PROBE"
        private const val STOP = "org.harmion.mote.voice.STOP_PROBE"
        private const val CHANNEL = "mote-voice-probe"
        private const val NOTIFICATION_ID = 301
        private val status = MutableStateFlow(VoiceProbeState())
        val state = status.asStateFlow()

        fun start(context: Context): Boolean {
            val notifications = context.getSystemService(NotificationManager::class.java)
            notifications.createNotificationChannel(
                NotificationChannel(CHANNEL, "録音検証", NotificationManager.IMPORTANCE_LOW),
            )
            if (!notificationsAvailable(notifications)) return false
            context.startForegroundService(Intent(context, VoiceProbeService::class.java).setAction(START))
            return true
        }

        private fun notificationsAvailable(notifications: NotificationManager): Boolean =
            notifications.areNotificationsEnabled() &&
                notifications.getNotificationChannel(CHANNEL)?.importance?.let { it > NotificationManager.IMPORTANCE_NONE } == true

        fun stop(context: Context, id: String) {
            context.startService(stopIntent(context, id))
        }

        private fun stopIntent(context: Context, id: String) =
            Intent(context, VoiceProbeService::class.java)
                .setAction(STOP)
                // PendingIntent identity includes data but ignores extras.
                .setData(Uri.Builder().scheme("mote-voice-probe").authority("recording").appendPath(id).build())
    }
}

private fun writeChunk(
    directory: File,
    id: String,
    startSample: Int,
    pcm: ByteArray,
    sampleCount: Int,
    remainingBytes: Long,
): Long {
    val dataBytes = sampleCount * Short.SIZE_BYTES
    val bytes = WAV_HEADER_BYTES + dataBytes
    if (bytes > remainingBytes) throw IOException("Recording quota")
    val header = ByteBuffer.allocate(WAV_HEADER_BYTES).order(ByteOrder.LITTLE_ENDIAN)
        .put("RIFF".toByteArray(Charsets.US_ASCII)).putInt(bytes - 8)
        .put("WAVEfmt ".toByteArray(Charsets.US_ASCII)).putInt(16)
        .putShort(1).putShort(1).putInt(PROBE_SAMPLE_RATE)
        .putInt(PROBE_SAMPLE_RATE * Short.SIZE_BYTES).putShort(2).putShort(16)
        .put("data".toByteArray(Charsets.US_ASCII)).putInt(dataBytes)
    val file = File(directory, "$id-${startSample.toString().padStart(10, '0')}.wav")
    val temporary = File(directory, "${file.name}.part")
    FileOutputStream(temporary).use {
        it.write(header.array())
        it.write(pcm, 0, dataBytes)
        it.fd.sync()
    }
    Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE)
    return remainingBytes - bytes
}
