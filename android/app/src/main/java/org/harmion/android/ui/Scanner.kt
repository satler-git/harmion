package org.harmion.android.ui

import com.journeyapps.barcodescanner.ScanOptions

/** Shared options for every Harmion QR scan entry point. */
internal fun harmionScanOptions(): ScanOptions =
    ScanOptions()
        .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
        .setCaptureActivity(HarmionScannerActivity::class.java)
        .setBeepEnabled(false)
        .setBarcodeImageEnabled(false)
        .setOrientationLocked(true)
