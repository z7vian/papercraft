package org.papercraft

// Pure rules for saving: no Android types here, so the JVM unit tests cover them.

/** What we know about one version of a file: size in bytes and last-modified time in ms; -1 (or 0 for the time) when the provider doesn't say. */
data class FileStamp(val size: Long, val modified: Long)

/**
 * True when [now] proves the file changed after [opened]. Values the provider doesn't report can't prove anything.
 * Limit: a provider that reports neither size nor last-modified hides every outside change, and one that reports only size hides same-size edits; lift it by hashing the bytes at open and before saving (costs a full read, a download on cloud providers).
 */
fun changedOnDisk(opened: FileStamp, now: FileStamp): Boolean =
    (opened.size >= 0 && now.size >= 0 && opened.size != now.size) ||
        (opened.modified > 0 && now.modified > 0 && opened.modified != now.modified)

/** How edited bytes of a file get written: as [ext] with [mime]. [inPlace] is false when the original's format can't be written back, so only "Save as" works. */
data class SaveTarget(val ext: String, val mime: String, val inPlace: Boolean)

private val saveMimes = mapOf(
    "docx" to "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "docm" to "application/vnd.ms-word.document.macroEnabled.12",
    "dotx" to "application/vnd.openxmlformats-officedocument.wordprocessingml.template",
    "dotm" to "application/vnd.ms-word.template.macroEnabled.12",
    "xlsx" to "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "xlsm" to "application/vnd.ms-excel.sheet.macroEnabled.12",
    "xltx" to "application/vnd.openxmlformats-officedocument.spreadsheetml.template",
    "xltm" to "application/vnd.ms-excel.template.macroEnabled.12",
    "xlam" to "application/vnd.ms-excel.addin.macroEnabled.12",
    "pptx" to "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "pptm" to "application/vnd.ms-powerpoint.presentation.macroEnabled.12",
    "potx" to "application/vnd.openxmlformats-officedocument.presentationml.template",
    "potm" to "application/vnd.ms-powerpoint.template.macroEnabled.12",
    "ppsx" to "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
    "ppsm" to "application/vnd.ms-powerpoint.slideshow.macroEnabled.12",
    "pdf" to "application/pdf",
)

/** Binary formats the core converts to OOXML when opening (office::convert); their edits save as the OOXML format. */
private val legacySaveExt = mapOf(
    "doc" to "docx", "dot" to "docx", "wps" to "docx", "wpt" to "docx",
    "xls" to "xlsx", "xlt" to "xlsx", "xla" to "xlsx", "et" to "xlsx", "ett" to "xlsx",
    "ppt" to "pptx", "pot" to "pptx", "pps" to "pptx", "dps" to "pptx", "dpt" to "pptx",
)

/** Where edits of a file with extension [ext] are saved, or null when Papercraft doesn't write that format. */
fun saveTargetOf(ext: String): SaveTarget? {
    val e = ext.lowercase()
    legacySaveExt[e]?.let { return SaveTarget(it, saveMimes.getValue(it), inPlace = false) }
    return saveMimes[e]?.let { SaveTarget(e, it, inPlace = true) }
}

/** The name "Save as" suggests: [name] with its [ext] (if it ends with it) replaced by [target]'s. */
fun suggestedName(name: String, ext: String, target: SaveTarget): String {
    val base = if (ext.isNotEmpty() && name.endsWith(".$ext", ignoreCase = true)) name.dropLast(ext.length + 1) else name
    return base.trim().ifEmpty { "Untitled" } + "." + target.ext
}
