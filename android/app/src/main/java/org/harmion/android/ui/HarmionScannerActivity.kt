package org.harmion.android.ui

import android.content.pm.ActivityInfo
import android.graphics.Color
import android.os.Bundle
import android.widget.ImageButton
import android.widget.Toast
import org.harmion.android.R
import com.journeyapps.barcodescanner.CaptureActivity
import com.journeyapps.barcodescanner.DecoratedBarcodeView
import com.journeyapps.barcodescanner.camera.CameraSettings

/**
 * Portrait QR scanner used by onboarding and peer approval.
 *
 * The library activity is declared as `sensorLandscape`; this activity keeps the
 * camera in portrait and adds a clear frame, a torch toggle and a single close
 * action. Continuous autofocus makes repeated scans feel faster than the
 * default one-shot focus.
 */
class HarmionScannerActivity : CaptureActivity() {
    private lateinit var scannerView: DecoratedBarcodeView
    private var torchOn = false

    override fun initializeContent(): DecoratedBarcodeView {
        setContentView(R.layout.activity_harmion_scanner)
        scannerView = findViewById(R.id.harmion_scanner_view)
        scannerView.getViewFinder().setMaskColor(Color.argb(168, 0, 0, 0))

        findViewById<ImageButton>(R.id.harmion_scanner_close).setOnClickListener { finish() }
        val torch = findViewById<ImageButton>(R.id.harmion_scanner_torch)
        torch.setOnClickListener {
            if (!scannerView.barcodeView.isPreviewActive) {
                Toast.makeText(this, "カメラを準備しています…", Toast.LENGTH_SHORT).show()
                return@setOnClickListener
            }
            torchOn = !torchOn
            runCatching {
                    if (torchOn) scannerView.setTorchOn() else scannerView.setTorchOff()
                }
                .onFailure {
                    torchOn = false
                    Toast.makeText(this, "この端末ではライトを使えません", Toast.LENGTH_SHORT).show()
                }
            torch.isSelected = torchOn
        }
        return scannerView
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
        // CaptureManager applies the intent defaults after initializeContent();
        // tune the camera here, before onResume opens it.
        scannerView.setCameraSettings(
            CameraSettings().apply {
                setFocusMode(CameraSettings.FocusMode.CONTINUOUS)
                setContinuousFocusEnabled(true)
                setAutoFocusEnabled(true)
                setMeteringEnabled(true)
                setBarcodeSceneModeEnabled(true)
            },
        )
    }
}
