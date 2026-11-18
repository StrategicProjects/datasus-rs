# Oracle test: the Rust reader (via the `datasus` CLI -> Parquet) must return
# exactly what datasusr's C reader returns, for the same options.
#
#   Rscript tests/oracle/compare.R <datasus binary> <helper-dbf.R> [files...]
#
# Needs the datasusr, arrow and testthat-free base R. Extra arguments are
# real .dbc/.dbf files to compare in addition to the synthetic ones.

suppressPackageStartupMessages({library(datasusr); library(arrow)})

# datasusr trims with C isspace(), which is locale-dependent: under a UTF-8
# locale macOS also treats 0xA0 (NBSP) as whitespace. The Rust core always
# uses the "C" locale definition, so compare under that locale.
invisible(Sys.setlocale("LC_CTYPE", "C"))
args <- commandArgs(trailingOnly = TRUE)
bin <- args[1]; source(args[2]); real <- args[-(1:2)]

norm <- function(x) {
  x <- as.data.frame(x, stringsAsFactors = FALSE)
  for (j in seq_along(x)) {
    if (inherits(x[[j]], "Date")) x[[j]] <- as.Date(as.numeric(x[[j]]))
    # R on macOS/Windows converts "latin1" strings as CP1252; the Rust core
    # decodes strict ISO-8859-1, so convert explicitly before comparing.
    if (is.character(x[[j]]) && any(Encoding(x[[j]]) == "latin1")) {
      x[[j]] <- iconv(x[[j]], "latin1", "UTF-8")
    }
  }
  x
}

# With encoding = "UTF-8" R keeps invalid byte sequences as they are while
# Arrow strings must be valid (U+FFFD is substituted): skip those values.
mask_invalid <- function(got, want) {
  for (j in seq_along(want)) if (is.character(want[[j]]) && j <= ncol(got)) {
    bad <- !validUTF8(want[[j]])
    if (any(bad) && is.character(got[[j]]) && length(got[[j]]) == length(bad)) {
      want[[j]][bad] <- NA; got[[j]][bad] <- NA
    }
  }
  list(got = got, want = want)
}

cli_args <- function(o) {
  a <- character()
  if (!is.null(o$select)) a <- c(a, "--select", paste(o$select, collapse = ","))
  if (!is.null(o$n_max)) a <- c(a, "--n-max", format(o$n_max, scientific = FALSE))
  if (isFALSE(o$trim_ws)) a <- c(a, "--no-trim")
  if (isFALSE(o$guess_types)) a <- c(a, "--no-guess-types")
  if (isTRUE(o$parse_dates)) a <- c(a, "--parse-dates")
  if (isFALSE(o$clean_names)) a <- c(a, "--keep-names")
  if (!is.null(o$encoding)) a <- c(a, "--encoding", o$encoding)
  for (n in names(o$col_types)) a <- c(a, "--col-type", paste0(n, "=", o$col_types[[n]]))
  a
}

n_ok <- 0L; n_bad <- 0L
check <- function(file, label, o = list()) {
  want <- tryCatch(
    suppressWarnings(do.call(read_datasus_dbc, c(list(file, verbose = FALSE), o))),
    error = function(e) e
  )
  out <- tempfile(fileext = ".parquet")
  rc <- suppressWarnings(system2(bin, c("convert", shQuote(file), "-o", shQuote(out), "-q", shQuote(cli_args(o))),
                                 stdout = FALSE, stderr = FALSE))
  if (inherits(want, "error")) {
    ok <- rc != 0; why <- "R errors but the CLI succeeded"
  } else if (rc != 0) {
    ok <- FALSE; why <- "CLI failed"
  } else {
    m <- mask_invalid(norm(read_parquet(out)), norm(want)); got <- m$got; want <- m$want
    ok <- identical(dim(got), dim(want)) && identical(names(got), names(want)) &&
      identical(vapply(got, typeof, ""), vapply(want, typeof, "")) &&
      identical(vapply(got, function(c) class(c)[1], ""), vapply(want, function(c) class(c)[1], "")) &&
      isTRUE(all.equal(got, want, tolerance = 0, check.attributes = FALSE))
    why <- if (ok) "" else {
      bad <- names(want)[!vapply(seq_along(want), function(j)
        j <= ncol(got) && identical(typeof(got[[j]]), typeof(want[[j]])) &&
          isTRUE(all.equal(got[[j]], want[[j]], tolerance = 0, check.attributes = FALSE)), NA)]
      paste("differs in:", paste(head(bad, 8), collapse = ", "))
    }
  }
  unlink(out)
  if (ok) n_ok <<- n_ok + 1L else {
    n_bad <<- n_bad + 1L
    cat(sprintf("FAIL %s [%s] %s\n", basename(file), label, why))
  }
  invisible(ok)
}

variants <- function(file) {
  hdr <- tryCatch(names(read_datasus_dbc(file, n_max = 1, verbose = FALSE, clean_names = FALSE)),
                  error = function(e) character())
  check(file, "default")
  check(file, "parse_dates", list(parse_dates = TRUE))
  check(file, "no guess", list(guess_types = FALSE))
  check(file, "no trim, keep names", list(trim_ws = FALSE, clean_names = FALSE))
  check(file, "n_max 7", list(n_max = 7))
  check(file, "utf-8", list(encoding = "UTF-8"))
  if (length(hdr) >= 3) {
    check(file, "select", list(select = c(tolower(hdr[3]), hdr[1], "nope")))
    ct <- setNames(c("character", "double", "date"), hdr[1:3])
    check(file, "col_types", list(col_types = ct))
    check(file, "all integer", list(col_types = setNames(rep("integer", length(hdr)), hdr)))
    check(file, "all logical", list(col_types = setNames(rep("logical", length(hdr)), hdr)))
    check(file, "all date", list(col_types = setNames(rep("date", length(hdr)), hdr)))
    check(file, "all double", list(col_types = setNames(rep("double", length(hdr)), hdr)))
  }
}

# --- synthetic files ---------------------------------------------------------
pad <- function(x, n) formatC(x, width = n, flag = "-"); padl <- function(x, n) formatC(x, width = n)
f6 <- list(list(name = "NAME", type = "C", len = 5), list(name = "CNT", type = "N", len = 4),
           list(name = "VAL", type = "N", len = 6, dec = 2), list(name = "RATE", type = "F", len = 6),
           list(name = "DT", type = "D", len = 8), list(name = "OK", type = "L", len = 1))
variants(make_dbf(f6, c(
  paste0(" ", pad("ana", 5), padl("12", 4), padl("3.50", 6), padl("1.5", 6), "20240115", "T"),
  paste0("*", pad("del", 5), padl("99", 4), padl("9.99", 6), padl("9", 6), "20200101", "T"),
  paste0(" ", pad("bruno", 5), padl("-7", 4), padl("10.25", 6), padl("2", 6), "20231231", "F"),
  paste0(" ", pad("", 5), padl("", 4), padl("", 6), padl("", 6), pad("", 8), "?"),
  paste0(" ", pad("caf\xe9", 5), padl("+3", 4), padl("1e2", 6), padl("inf", 6), "20240230", "y"),
  paste0(" ", pad(" x ", 5), padl("1.5", 4), padl("abc", 6), padl(".", 6), "2024011 ", "n"))))
fn <- list(list(name = "N", type = "N", len = 12), list(name = "M", type = "N", len = 12),
           list(name = "T", type = "C", len = 8))
variants(make_dbf(fn, c(
  paste0(" ", padl("2147483648", 12), padl("2147483647", 12), "20240229"),
  paste0(" ", padl("-2147483648", 12), padl("-2147483647", 12), "19000229"),
  paste0(" ", padl("99999999999", 12), padl("007", 12), " 1999123"),
  paste0(" ", padl("1,5", 12), padl("- 1", 12), "TRUE    "))))
variants(make_dbf(list(list(name = "ID", type = "N", len = 3)), c("   1", "   2"), n_records = 5L))
variants(make_dbf(list(list(name = "ID", type = "N", len = 3)), c("*  1", "*  2")))
check(make_dbf(list(list(name = "A", type = "C", len = 200), list(name = "B", type = "C", len = 200)),
               strrep(" ", 11), record_len = 11L), "bad lengths")
junk <- tempfile(fileext = ".dbc"); writeBin(as.raw(c(0xFF, 0x00, 0x13, 0x37, 0x42)), junk)
check(junk, "garbage")

# --- real files --------------------------------------------------------------
for (f in real) { cat("..", f, "\n"); variants(f) }

cat(sprintf("\n%d comparisons identical, %d differ\n", n_ok, n_bad))
quit(status = if (n_bad > 0) 1 else 0)
