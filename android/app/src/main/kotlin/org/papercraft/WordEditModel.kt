package org.papercraft

// The Word editor's pure rules: no Android or Compose types, so the JVM unit tests cover them.
// Offsets are UTF-16 units (Kotlin String indices), the unit DocEditor counts in.

import org.papercraft.core.DocAlign
import org.papercraft.core.DocBlock
import org.papercraft.core.DocParagraph
import org.papercraft.core.DocRun
import org.papercraft.core.DocRunFormat
import org.papercraft.core.DocRunProperty

/** One run placed in its paragraph's text, at [start, end). */
internal data class WordSpan(val start: Int, val end: Int, val run: DocRun)

/** Text at [start, end) inside a link, field, content control or tracked change: shown, not editable. */
internal data class WordLock(val start: Int, val end: Int)

/**
 * A paragraph as the editor shows it. [index] numbers it as DocEditor does. [heading] is 0 for a title, 1–9 for heading
 * levels, null otherwise. [joinsPrevious]/[joinsNext]: a paragraph sits right before/after it in the same body or cell, so
 * Backspace at its start / Delete at its end can join the two.
 */
internal data class WordPara(
    val index: Int,
    val text: String,
    val heading: Int?,
    val listLevel: Int?,
    val align: DocAlign,
    val styleId: String?,
    val styleName: String?,
    val spans: List<WordSpan>,
    val locks: List<WordLock>,
    val joinsPrevious: Boolean,
    val joinsNext: Boolean,
)

internal sealed interface WordBlock {
    data class Para(val para: WordPara) : WordBlock

    /** Rows of cells, each cell its paragraphs. */
    data class Table(val rows: List<List<List<WordPara>>>) : WordBlock
}

/** The document body as the editor shows it, and every paragraph by its index. */
internal class WordModel(val blocks: List<WordBlock>) {
    val paragraphs: Map<Int, WordPara>
    private val blockIndex: Map<Int, Int>

    init {
        val paragraphs = HashMap<Int, WordPara>()
        val blockIndex = HashMap<Int, Int>()
        fun add(p: WordPara, block: Int) {
            paragraphs[p.index] = p
            blockIndex[p.index] = block
        }
        blocks.forEachIndexed { b, block ->
            when (block) {
                is WordBlock.Para -> add(block.para, b)
                is WordBlock.Table -> block.rows.forEach { row -> row.forEach { cell -> cell.forEach { add(it, b) } } }
            }
        }
        this.paragraphs = paragraphs
        this.blockIndex = blockIndex
    }

    /** The block paragraph [index] sits in: itself, or its table. */
    fun blockOf(index: Int): Int? = blockIndex[index]
}

/** [blocks] from DocEditor.blocks as the editor shows them. */
internal fun wordModel(blocks: List<DocBlock>): WordModel = WordModel(
    blocks.mapIndexed { i, block ->
        when (block) {
            is DocBlock.Paragraph -> WordBlock.Para(
                wordPara(
                    block.paragraph,
                    joinsPrevious = blocks.getOrNull(i - 1) is DocBlock.Paragraph,
                    joinsNext = blocks.getOrNull(i + 1) is DocBlock.Paragraph,
                ),
            )
            is DocBlock.Table -> WordBlock.Table(
                block.rows.map { row ->
                    row.cells.map { cell ->
                        cell.paragraphs.mapIndexed { j, p -> wordPara(p, joinsPrevious = j > 0, joinsNext = j < cell.paragraphs.size - 1) }
                    }
                },
            )
        }
    },
)

private fun wordPara(p: DocParagraph, joinsPrevious: Boolean, joinsNext: Boolean): WordPara {
    val spans = ArrayList<WordSpan>(p.runs.size)
    var at = 0
    for (run in p.runs) {
        spans += WordSpan(at, at + run.text.length, run)
        at += run.text.length
    }
    return WordPara(
        index = p.index.toInt(),
        text = p.text,
        heading = headingLevel(p.styleId, p.styleName),
        listLevel = p.listLevel?.toInt(),
        align = p.alignment,
        styleId = p.styleId,
        styleName = p.styleName,
        spans = spans,
        locks = spans.filter { !it.run.editable }.map { WordLock(it.start, it.end) },
        joinsPrevious = joinsPrevious,
        joinsNext = joinsNext,
    )
}

private val headingName = Regex("""heading\s*([1-9])""", RegexOption.IGNORE_CASE)

/** 0 for Word's Title style, 1–9 for its Heading styles, null for others. Built-in styles carry English names, so the name decides first. */
internal fun headingLevel(styleId: String?, styleName: String?): Int? {
    for (name in listOfNotNull(styleName, styleId)) {
        if (name.equals("title", ignoreCase = true)) return 0
        headingName.matchEntire(name.trim())?.let { return it.groupValues[1].toInt() }
    }
    return null
}

/** One edit of a paragraph's text: [start, end) of the old text replaced by [text]. */
internal data class FieldChange(val start: Int, val end: Int, val text: String) {
    /** How much longer the text got. */
    val delta: Int get() = text.length - (end - start)

    /**
     * This change followed by [next] (in the text this one produced) as one change, or null when [next] reaches outside
     * the text this one inserted: then the two go to the engine separately.
     */
    fun then(next: FieldChange): FieldChange? {
        if (next.start < start || next.end > start + text.length) return null
        return FieldChange(start, end, text.substring(0, next.start - start) + next.text + text.substring(next.end - start))
    }

    /** True when this change starts or ends strictly inside one of [locks], which the engine refuses. */
    fun cuts(locks: List<WordLock>): Boolean = locks.any { start > it.start && start < it.end || end > it.start && end < it.end }
}

/**
 * The change that turns [old] into [new] in a text field whose caret ended at [cursor]: the edited stretch ends at or
 * before the caret, which settles where repeated letters were typed. Null when the text is the same. Never cuts a
 * surrogate pair, which the engine refuses.
 */
internal fun fieldChange(old: String, new: String, cursor: Int): FieldChange? {
    if (old == new) return null
    var suffix = 0
    val maxSuffix = minOf(old.length, new.length - cursor.coerceIn(0, new.length))
    while (suffix < maxSuffix && old[old.length - 1 - suffix] == new[new.length - 1 - suffix]) suffix++
    var prefix = 0
    val maxPrefix = minOf(old.length, new.length) - suffix
    while (prefix < maxPrefix && old[prefix] == new[prefix]) prefix++
    if (prefix > 0 && old[prefix - 1].isHighSurrogate()) prefix--
    if (suffix > 0 && old[old.length - suffix].isLowSurrogate()) suffix--
    return FieldChange(prefix, old.length - suffix, new.substring(prefix, new.length - suffix))
}

/** [locks] after [change]: later ones move with the text, ones it removed go. */
internal fun List<WordLock>.after(change: FieldChange): List<WordLock> = mapNotNull {
    when {
        it.end <= change.start -> it
        it.start >= change.end -> WordLock(it.start + change.delta, it.end + change.delta)
        else -> null
    }
}

/**
 * A change typed with Enter (or pasted with line ends): the same change without its newlines, and the offsets in the
 * resulting paragraph where it splits, in ascending order. Null when [change] has no newline.
 */
internal fun splitAtNewlines(change: FieldChange): Pair<FieldChange, List<Int>>? {
    if ('\n' !in change.text) return null
    val kept = StringBuilder(change.text.length)
    val cuts = ArrayList<Int>()
    for (c in change.text) if (c == '\n') cuts += change.start + kept.length else kept.append(c)
    return change.copy(text = kept.toString()) to cuts
}

/** Keys the paragraph field handles itself; other keys type as usual. */
internal enum class WordKey { Enter, Backspace, Delete }

internal enum class WordKeyAction {
    /** Nothing special: the field handles the key (Enter types a newline, which splits the paragraph). */
    None,

    /** Shift+Enter: a line break inside the paragraph. */
    LineBreak,

    /** Backspace at the very start: join with the paragraph before. */
    JoinPrevious,

    /** Delete at the very end: join with the paragraph after. */
    JoinNext,
}

/** What [key] (with [shift]) does in [para]'s field with the selection [selStart]..[selEnd]. */
internal fun keyAction(key: WordKey, shift: Boolean, selStart: Int, selEnd: Int, para: WordPara): WordKeyAction {
    val collapsed = selStart == selEnd
    return when {
        key == WordKey.Enter && shift -> WordKeyAction.LineBreak
        key == WordKey.Backspace && collapsed && selStart == 0 && para.joinsPrevious -> WordKeyAction.JoinPrevious
        key == WordKey.Delete && collapsed && selEnd == para.text.length && para.joinsNext -> WordKeyAction.JoinNext
        else -> WordKeyAction.None
    }
}

/** Character formatting of a selection; a field is null where its runs differ. */
internal data class WordLook(
    val bold: Boolean?,
    val italic: Boolean?,
    val underline: Boolean?,
    val strike: Boolean?,
    val sizePt: Float?,
    val color: String?,
    val font: String?,
)

/** The formatting every run in [start, end) of [spans] shares. */
internal fun selectionLook(spans: List<WordSpan>, start: Int, end: Int): WordLook {
    val runs = spans.filter { it.end > start && it.start < end }.map { it.run }
    fun <T> shared(field: (DocRun) -> T): T? = runs.map(field).distinct().singleOrNull()
    return WordLook(
        bold = shared { it.bold }, italic = shared { it.italic }, underline = shared { it.underline }, strike = shared { it.strike },
        sizePt = shared { it.sizePt }, color = shared { it.color }, font = shared { it.font },
    )
}

/** The one place a [DocRunFormat] is built: null leaves a property as it is, [clear] puts properties back to the styles' look. */
internal fun runFormat(
    bold: Boolean? = null,
    italic: Boolean? = null,
    underline: Boolean? = null,
    strike: Boolean? = null,
    sizePt: Float? = null,
    color: String? = null,
    clear: List<DocRunProperty> = emptyList(),
) = DocRunFormat(
    bold = bold, italic = italic, underline = underline, strike = strike, sizePt = sizePt, color = color, font = null, clear = clear,
)

/** Direct character formatting removed: the text takes its paragraph and character styles' look again. */
internal fun clearFormat() = runFormat(clear = DocRunProperty.entries.toList())

/** The generic family that stands in for a document font on the phone. */
internal enum class WordFont { Sans, Serif, Mono }

private val monoFonts = listOf("courier", "consolas", "mono", "menlo", "lucida console")
private val serifFonts = listOf("times", "georgia", "cambria", "garamond", "serif", "book antiqua", "palatino", "baskerville", "minion", "song", "mincho")

/**
 * The family for document font [name].
 * Limit: documents show in the phone's generic families; loading the fonts a file names (or embeds) lifts it.
 */
internal fun wordFont(name: String?): WordFont {
    val n = name?.lowercase() ?: return WordFont.Sans
    return when {
        monoFonts.any { it in n } -> WordFont.Mono
        // "sans serif" names say serif too.
        "sans" !in n && serifFonts.any { it in n } -> WordFont.Serif
        else -> WordFont.Sans
    }
}

/** `RRGGBB` as an opaque ARGB colour, or null when it isn't one. */
internal fun argbOf(hex: String?): Long? {
    val h = hex?.removePrefix("#") ?: return null
    if (h.length != 6) return null
    return h.toLongOrNull(16)?.let { 0xFF000000L or it }
}

/** [start, end) widened so neither end falls inside one of [locks], which formatting can't cut. */
internal fun outsideLocks(start: Int, end: Int, locks: List<WordLock>): Pair<Int, Int> {
    val s = locks.firstOrNull { start > it.start && start < it.end }?.start ?: start
    val e = locks.firstOrNull { end > it.start && end < it.end }?.end ?: end
    return s to e
}
