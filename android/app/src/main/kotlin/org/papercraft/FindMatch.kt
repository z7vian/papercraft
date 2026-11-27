package org.papercraft

import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

// Limit: one search keeps at most this many hits, so a one-letter query on a huge file stays small in memory; store hits as packed ints to lift it.
const val MAX_FIND_HITS = 5_000

/** A match in a text viewer: characters [start] until [end] of line [line]. */
data class TextHit(val line: Int, val start: Int, val end: Int)

/** A box on a drawn page as fractions of the page's width and height, so it scales with the page at any zoom. */
data class PageBox(val left: Float, val top: Float, val right: Float, val bottom: Float) {
    /** This box in pixels on a page drawn [width] x [height] pixels. */
    fun scaled(width: Float, height: Float) = PageBox(left * width, top * height, right * width, bottom * height)

    companion object {
        /** A box given in PDF points from the top-left corner of a page [pageWidth] x [pageHeight] points. */
        fun fromPoints(left: Float, top: Float, right: Float, bottom: Float, pageWidth: Float, pageHeight: Float) =
            PageBox(left / pageWidth, top / pageHeight, right / pageWidth, bottom / pageHeight)
    }
}

/** A match on page [page]: one box per line it spans, or none when the file only says which page holds it. */
data class PageHit(val page: Int, val boxes: List<PageBox>)

/**
 * Every match of [query] in [lines] in reading order, at most [limit]: case-insensitive, accent-sensitive ("e" doesn't find "é").
 * Matches may overlap ("aa" finds two in "aaa") and stay within one line. Stops between lines once its coroutine is cancelled.
 */
// Limit: a match split across two of the 4000-character pieces `loadText` cuts giant lines into isn't found; search the unsplit text to lift it.
suspend fun findInLines(lines: List<String>, query: String, limit: Int = MAX_FIND_HITS): List<TextHit> {
    if (query.isEmpty()) return emptyList()
    val context = currentCoroutineContext()
    val hits = ArrayList<TextHit>()
    for ((i, line) in lines.withIndex()) {
        context.ensureActive()
        // Char-by-char case folding keeps every index the same in the line and the match.
        var at = line.indexOf(query, 0, ignoreCase = true)
        while (at >= 0) {
            hits += TextHit(i, at, at + query.length)
            if (hits.size >= limit) return hits
            at = line.indexOf(query, at + 1, ignoreCase = true)
        }
    }
    return hits
}

/** The hit after [current] of [count], wrapping to the first; -1 when there are none. */
fun nextHit(current: Int, count: Int): Int = if (count <= 0) -1 else (current + 1).mod(count)

/** The hit before [current] of [count], wrapping to the last; -1 when there are none. */
fun previousHit(current: Int, count: Int): Int = if (count <= 0) -1 else if (current <= 0 || current > count) count - 1 else current - 1

/** The first of these hits (in reading order) whose [position] is at or after [from], else the first one; -1 when empty. */
inline fun <T> List<T>.firstAtOrAfter(from: Int, position: (T) -> Int): Int =
    if (isEmpty()) -1 else indexOfFirst { position(it) >= from }.coerceAtLeast(0)

/** The find bar's "n of m": [pagesOnly] counts pages holding matches; null while there is no query. */
fun findLabel(query: String, searching: Boolean, failed: Boolean, current: Int, count: Int, capped: Boolean, pagesOnly: Boolean): String? = when {
    query.isEmpty() -> null
    searching -> "Searching…"
    failed -> "Can't search this file"
    count == 0 -> "No matches"
    else -> "${current + 1} of $count" + (if (capped) "+" else "") + (if (pagesOnly) " pages" else "")
}
