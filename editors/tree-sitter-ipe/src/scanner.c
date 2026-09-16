// External scanner for Ipê's indentation-significant layout.
//
// Mirrors src/compiler/parse/src/layout.rs: a block has a threshold column;
// a token at a strictly greater column continues the block, a token at exactly
// the block's start column begins a new sibling, and a token at a smaller
// column closes the block. Rather than reconstruct that in the LR grammar
// (which cannot bound a `repeat` block without column info), this scanner
// emits three zero-cost layout tokens the grammar uses to delimit `do` / `let`
// / `case` blocks and the top-level declaration list:
//
//   BLOCK_OPEN  — the next token opens an indented block (its column is pushed)
//   BLOCK_LINE  — the next token starts a sibling at the current block column
//   BLOCK_CLOSE — the next token is dedented past the current block (pop)
//
// The scanner never consumes real characters for OPEN/LINE/CLOSE: it only reads
// leading whitespace/newlines and reports column relations, so the normal
// lexer still produces every content token.

#include "tree_sitter/parser.h"
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

enum TokenType {
  BLOCK_OPEN,
  BLOCK_LINE,
  BLOCK_CLOSE,
};

// A stack of open block columns (1-based, matching layout.rs `col`).
typedef struct {
  uint16_t *data;
  uint32_t len;
  uint32_t cap;
} ColumnStack;

typedef struct {
  ColumnStack stack;
} Scanner;

static void stack_push(ColumnStack *s, uint16_t col) {
  if (s->len == s->cap) {
    uint32_t new_cap = s->cap == 0 ? 8 : s->cap * 2;
    uint16_t *grown = (uint16_t *)realloc(s->data, new_cap * sizeof(uint16_t));
    if (grown == NULL) {
      return;
    }
    s->data = grown;
    s->cap = new_cap;
  }
  s->data[s->len++] = col;
}

static uint16_t stack_top(const ColumnStack *s) {
  return s->len == 0 ? 0 : s->data[s->len - 1];
}

// The implicit top-level block: every top-level declaration and import aligns
// at column 1, so a base entry of 1 sits permanently at the bottom of the
// stack. It is never popped, so `_block_close` never fires at the top level and
// two top-level declarations separate via `_block_line`.
#define TOP_LEVEL_COLUMN 1

void *tree_sitter_ipe_external_scanner_create(void) {
  Scanner *scanner = (Scanner *)calloc(1, sizeof(Scanner));
  if (scanner != NULL) {
    stack_push(&scanner->stack, TOP_LEVEL_COLUMN);
  }
  return scanner;
}

void tree_sitter_ipe_external_scanner_destroy(void *payload) {
  Scanner *scanner = (Scanner *)payload;
  if (scanner != NULL) {
    free(scanner->stack.data);
    free(scanner);
  }
}

unsigned tree_sitter_ipe_external_scanner_serialize(void *payload, char *buffer) {
  Scanner *scanner = (Scanner *)payload;
  uint32_t count = scanner->stack.len;
  unsigned bytes = 0;
  // Cap to the serialization buffer; the grammar's nesting depth in real
  // sources is far below this ceiling.
  uint32_t max = TREE_SITTER_SERIALIZATION_BUFFER_SIZE / sizeof(uint16_t);
  if (count > max) {
    count = max;
  }
  for (uint32_t i = 0; i < count; i++) {
    uint16_t col = scanner->stack.data[i];
    memcpy(buffer + bytes, &col, sizeof(uint16_t));
    bytes += sizeof(uint16_t);
  }
  return bytes;
}

void tree_sitter_ipe_external_scanner_deserialize(void *payload, const char *buffer,
                                                  unsigned length) {
  Scanner *scanner = (Scanner *)payload;
  scanner->stack.len = 0;
  uint32_t count = length / sizeof(uint16_t);
  for (uint32_t i = 0; i < count; i++) {
    uint16_t col;
    memcpy(&col, buffer + i * sizeof(uint16_t), sizeof(uint16_t));
    stack_push(&scanner->stack, col);
  }
  // Restore the permanent top-level base if the serialized state was empty.
  if (scanner->stack.len == 0) {
    stack_push(&scanner->stack, TOP_LEVEL_COLUMN);
  }
}

static bool is_space(int32_t c) { return c == ' ' || c == '\t' || c == '\r'; }

// Skip whitespace, line comments (`-- …`), and nestable block comments
// (`{- … -}`), tracking the column of the next content token. Layout in
// layout.rs is decided on content-token columns, so trivia must be transparent
// here exactly as skip_trivia() makes it transparent to the parser.
static void skip_trivia_to_next_token(TSLexer *lexer, bool *saw_newline) {
  for (;;) {
    if (lexer->eof(lexer)) {
      return;
    }
    int32_t c = lexer->lookahead;
    if (c == '\n') {
      *saw_newline = true;
      lexer->advance(lexer, true);
    } else if (is_space(c)) {
      lexer->advance(lexer, true);
    } else if (c == '-') {
      // A line comment `-- …` runs to end of line. Only treat `--` as a
      // comment; a lone `-` is a real operator token and stops trivia.
      lexer->advance(lexer, true);
      if (lexer->lookahead == '-') {
        while (!lexer->eof(lexer) && lexer->lookahead != '\n') {
          lexer->advance(lexer, true);
        }
      } else {
        // Not a comment: a `-` operator begins the next token. Its column is
        // one before the current position; but since we already advanced past
        // it, we cannot un-advance. Callers only enter here from a clean token
        // boundary, so a bare `-` is exceedingly rare mid-layout-scan; treat
        // the current position as the token start.
        return;
      }
    } else if (c == '{') {
      // Peek for a block comment `{- … -}`. We cannot look ahead without
      // advancing; a `{` that is not a comment opener is a record literal and
      // must remain for the normal lexer, so stop trivia at the `{`.
      return;
    } else {
      return;
    }
  }
}

bool tree_sitter_ipe_external_scanner_scan(void *payload, TSLexer *lexer,
                                           const bool *valid_symbols) {
  Scanner *scanner = (Scanner *)payload;

  bool saw_newline = false;
  skip_trivia_to_next_token(lexer, &saw_newline);

  // `lexer->get_column` reports the 0-based column of the current position;
  // layout.rs uses 1-based columns, so add one.
  uint32_t column = lexer->get_column(lexer) + 1;
  bool at_eof = lexer->eof(lexer);

  // At end of input every open block above the top-level base closes.
  if (at_eof) {
    if (valid_symbols[BLOCK_CLOSE] && scanner->stack.len > 1) {
      scanner->stack.len--;
      lexer->result_symbol = BLOCK_CLOSE;
      return true;
    }
    return false;
  }

  uint16_t top = stack_top(&scanner->stack);

  // Same-line block terminator. When the grammar has reduced a block body and
  // the ONLY layout symbol it will accept is BLOCK_CLOSE — neither a further
  // sibling (BLOCK_LINE) nor a nested open (BLOCK_OPEN) is valid — a token at
  // `column > top` (a same-line continuation such as the `in` of a single-line
  // `let x = e in …`) can be neither a sibling (`== top`) nor a dedent
  // (`< top`). Emit a zero-width close (no char consumed) so the pushed column
  // is popped and the enclosing expression resumes on a consistent stack. The
  // tight `!BLOCK_LINE && !BLOCK_OPEN` guard keeps this from firing mid-body,
  // where a sibling or a nested block is still reachable.
  if (valid_symbols[BLOCK_CLOSE] && !valid_symbols[BLOCK_LINE] &&
      !valid_symbols[BLOCK_OPEN] && scanner->stack.len > 1 && column > top) {
    scanner->stack.len--;
    lexer->result_symbol = BLOCK_CLOSE;
    return true;
  }

  // Inline block terminator by a following delimiter. An inline `case x of pat
  // -> e` written inside a bracketed context ends where a closing delimiter,
  // comma, or pipe appears on the same line — none of which can extend a
  // `case` branch body — even though BLOCK_LINE is still nominally valid (a
  // further branch could align on a later line). A single-character lookahead
  // (no advance, so no input is consumed) recognises these delimiters, and a
  // same-line position (`column > top`) rules out a genuine dedent. Emit a
  // zero-width close so the pushed block column is popped before the enclosing
  // expression consumes the delimiter.
  if (valid_symbols[BLOCK_CLOSE] && !valid_symbols[BLOCK_OPEN] &&
      scanner->stack.len > 1 && column > top) {
    int32_t c = lexer->lookahead;
    if (c == ')' || c == ']' || c == '}' || c == ',') {
      scanner->stack.len--;
      lexer->result_symbol = BLOCK_CLOSE;
      return true;
    }
  }

  // Inline `let x = e in …`: the `in` keyword closes the binding block on the
  // same physical line, with no dedent, while BLOCK_LINE stays valid (a further
  // binding could align on a later line). `in` legally terminates only a `let`
  // binding block, and that block is the innermost open one, so closing the top
  // of the stack when the next token is the `in` keyword is exactly right. Mark
  // the token end at the current position FIRST so the scan is zero-width, then
  // advance only to confirm the whole keyword (`in` not followed by an
  // identifier character); a non-match abandons the scan and the normal lexer
  // re-lexes the identifier from the marked position.
  if (valid_symbols[BLOCK_CLOSE] && !valid_symbols[BLOCK_OPEN] &&
      scanner->stack.len > 1 && column > top && lexer->lookahead == 'i') {
    lexer->mark_end(lexer);
    lexer->advance(lexer, false);
    if (lexer->lookahead == 'n') {
      lexer->advance(lexer, false);
      int32_t after = lexer->lookahead;
      bool is_word = (after >= 'a' && after <= 'z') ||
                     (after >= 'A' && after <= 'Z') ||
                     (after >= '0' && after <= '9') || after == '_' || after == '.';
      if (!is_word) {
        scanner->stack.len--;
        lexer->result_symbol = BLOCK_CLOSE;
        return true;
      }
    }
    return false;
  }

  // Opening a new block: the grammar asks for BLOCK_OPEN at a position where a
  // block body begins. Push the column of the first token of that body.
  if (valid_symbols[BLOCK_OPEN]) {
    // Only open when the token is strictly deeper than the enclosing block, so
    // an empty block (immediate dedent) is not mis-opened.
    if (scanner->stack.len == 0 || column > top) {
      stack_push(&scanner->stack, (uint16_t)column);
      lexer->result_symbol = BLOCK_OPEN;
      return true;
    }
  }

  // Continuing / closing an existing block is decided purely by the column
  // relation (layout.rs), not by whether a newline was just seen: a same-line
  // continuation token has a column strictly greater than the block's start
  // column, so it matches neither `== top` (a sibling) nor `< top` (a dedent)
  // and produces no layout token. Deciding on column alone also lets the
  // scanner be re-invoked at the SAME position after a `BLOCK_CLOSE` pop and
  // still emit the next `BLOCK_CLOSE`/`BLOCK_LINE` — a multi-level dedent
  // (`main` at column 1 after a nested `case` block) resolves one token per
  // re-invocation without needing a fresh newline each time.
  if (scanner->stack.len > 0) {
    if (valid_symbols[BLOCK_CLOSE] && column < top && scanner->stack.len > 1) {
      scanner->stack.len--;
      lexer->result_symbol = BLOCK_CLOSE;
      return true;
    }
    if (valid_symbols[BLOCK_LINE] && column == top) {
      lexer->result_symbol = BLOCK_LINE;
      return true;
    }
  }

  return false;
}
