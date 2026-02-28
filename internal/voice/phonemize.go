//go:build cgo

package voice

import (
	"regexp"
	"strings"
	"unicode"
)

// Phonemize converts English text to a sequence of Kokoro token IDs.
// This is a simplified port of the Python ttstokenizer library,
// using rule-based English grapheme-to-phoneme conversion.
func Phonemize(text string) []int64 {
	text = strings.TrimSpace(text)
	if text == "" {
		return nil
	}

	// Normalize text
	text = normalizeText(text)

	// Convert to phonemes
	phonemes := textToPhonemes(text)
	if phonemes == "" {
		return nil
	}

	// Convert phonemes to token IDs
	return phonemesToTokens(phonemes)
}

// normalizeText cleans up input text for phonemization.
func normalizeText(text string) string {
	// Expand common abbreviations
	for _, pair := range abbreviations {
		text = strings.ReplaceAll(text, pair[0], pair[1])
	}

	// Normalize whitespace
	text = strings.Join(strings.Fields(text), " ")

	return text
}

// textToPhonemes converts English text to IPA phoneme string.
func textToPhonemes(text string) string {
	var result strings.Builder

	words := tokenizeText(text)
	for i, word := range words {
		if i > 0 {
			result.WriteRune(' ')
		}

		lower := strings.ToLower(word)

		// Check dictionary first
		if ph, ok := cmuDict[lower]; ok {
			result.WriteString(ph)
			continue
		}

		// Check if it's punctuation
		if len(word) == 1 && isPunctuation(rune(word[0])) {
			result.WriteString(punctuationPhoneme(rune(word[0])))
			continue
		}

		// Fall back to rule-based conversion
		result.WriteString(rulesBasedG2P(lower))
	}

	return result.String()
}

// tokenizeText splits text into words and punctuation tokens.
func tokenizeText(text string) []string {
	var tokens []string
	var current strings.Builder

	for _, r := range text {
		if unicode.IsLetter(r) || r == '\'' {
			current.WriteRune(r)
		} else {
			if current.Len() > 0 {
				tokens = append(tokens, current.String())
				current.Reset()
			}
			if isPunctuation(r) {
				tokens = append(tokens, string(r))
			}
		}
	}
	if current.Len() > 0 {
		tokens = append(tokens, current.String())
	}

	return tokens
}

func isPunctuation(r rune) bool {
	return r == '.' || r == ',' || r == '!' || r == '?' || r == ';' || r == ':' || r == '-'
}

func punctuationPhoneme(r rune) string {
	switch r {
	case '.', '!', '?':
		return "."
	case ',', ';', ':':
		return ","
	case '-':
		return " "
	}
	return ""
}

// phonemesToTokens maps an IPA phoneme string to Kokoro token IDs.
func phonemesToTokens(phonemes string) []int64 {
	var tokens []int64

	// Start token
	tokens = append(tokens, 0)

	for _, r := range phonemes {
		if id, ok := phonemeToID[string(r)]; ok {
			tokens = append(tokens, id)
		}
		// Unknown phonemes are silently skipped
	}

	// End token
	tokens = append(tokens, 0)

	return tokens
}

// rulesBasedG2P applies English grapheme-to-phoneme rules.
func rulesBasedG2P(word string) string {
	var result strings.Builder
	i := 0

	for i < len(word) {
		matched := false

		// Try multi-character rules first (longest match)
		for length := 4; length >= 2; length-- {
			if i+length > len(word) {
				continue
			}
			substr := word[i : i+length]
			if ph, ok := g2pRules[substr]; ok {
				result.WriteString(ph)
				i += length
				matched = true
				break
			}
		}

		if !matched {
			// Single character fallback
			ch := string(word[i])
			if ph, ok := g2pRules[ch]; ok {
				result.WriteString(ph)
			}
			i++
		}
	}

	return result.String()
}

// Regex for number detection
var numberRegex = regexp.MustCompile(`\d+`)

// abbreviations maps common abbreviations to their expansions.
var abbreviations = [][2]string{
	{"Mr.", "Mister"},
	{"Mrs.", "Missus"},
	{"Dr.", "Doctor"},
	{"St.", "Saint"},
	{"Jr.", "Junior"},
	{"Sr.", "Senior"},
	{"vs.", "versus"},
	{"etc.", "etcetera"},
	{"approx.", "approximately"},
	{"dept.", "department"},
	{"est.", "established"},
	{"govt.", "government"},
	{"e.g.", "for example"},
	{"i.e.", "that is"},
}

// g2pRules maps English grapheme sequences to IPA phonemes.
var g2pRules = map[string]string{
	// Multi-character rules
	"tion": "ʃən",
	"sion": "ʒən",
	"ough": "ʌf",
	"ight": "aɪt",
	"eous": "iəs",
	"ious": "iəs",
	"ture": "tʃɚ",
	"sure": "ʃɚ",
	"ould": "ʊd",
	"ound": "aʊnd",
	"ence": "əns",
	"ance": "əns",
	"ment": "mənt",
	"ness": "nəs",
	"able": "əbəl",
	"ible": "əbəl",
	"ally": "əli",
	"ful":  "fəl",
	"ing":  "ɪŋ",
	"ght":  "t",
	"tch":  "tʃ",
	"dge":  "dʒ",
	"sch":  "sk",
	"chr":  "kɹ",
	"que":  "k",
	"ph":   "f",
	"th":   "θ",
	"sh":   "ʃ",
	"ch":   "tʃ",
	"wh":   "w",
	"wr":   "ɹ",
	"kn":   "n",
	"gn":   "n",
	"ck":   "k",
	"ng":   "ŋ",
	"gh":   "",
	"ee":   "i",
	"ea":   "i",
	"oo":   "u",
	"ou":   "aʊ",
	"ow":   "oʊ",
	"ai":   "eɪ",
	"ay":   "eɪ",
	"oi":   "ɔɪ",
	"oy":   "ɔɪ",
	"au":   "ɔ",
	"aw":   "ɔ",
	"er":   "ɚ",
	"ir":   "ɝ",
	"ur":   "ɝ",
	"ar":   "ɑɹ",
	"or":   "ɔɹ",
	"le":   "əl",

	// Single character rules
	"a": "æ",
	"b": "b",
	"c": "k",
	"d": "d",
	"e": "ɛ",
	"f": "f",
	"g": "ɡ",
	"h": "h",
	"i": "ɪ",
	"j": "dʒ",
	"k": "k",
	"l": "l",
	"m": "m",
	"n": "n",
	"o": "ɑ",
	"p": "p",
	"q": "k",
	"r": "ɹ",
	"s": "s",
	"t": "t",
	"u": "ʌ",
	"v": "v",
	"w": "w",
	"x": "ks",
	"y": "j",
	"z": "z",
}

// cmuDict contains a small set of common English words with their IPA pronunciations.
// A full CMU dictionary would be loaded from file; this covers the most frequent words.
var cmuDict = map[string]string{
	"the":      "ðə",
	"a":        "ə",
	"an":       "ən",
	"and":      "ænd",
	"or":       "ɔɹ",
	"is":       "ɪz",
	"are":      "ɑɹ",
	"was":      "wɑz",
	"were":     "wɝ",
	"be":       "bi",
	"been":     "bɪn",
	"being":    "biɪŋ",
	"have":     "hæv",
	"has":      "hæz",
	"had":      "hæd",
	"do":       "du",
	"does":     "dʌz",
	"did":      "dɪd",
	"will":     "wɪl",
	"would":    "wʊd",
	"could":    "kʊd",
	"should":   "ʃʊd",
	"may":      "meɪ",
	"might":    "maɪt",
	"shall":    "ʃæl",
	"can":      "kæn",
	"must":     "mʌst",
	"i":        "aɪ",
	"you":      "ju",
	"he":       "hi",
	"she":      "ʃi",
	"it":       "ɪt",
	"we":       "wi",
	"they":     "ðeɪ",
	"me":       "mi",
	"him":      "hɪm",
	"her":      "hɝ",
	"us":       "ʌs",
	"them":     "ðɛm",
	"my":       "maɪ",
	"your":     "jɔɹ",
	"his":      "hɪz",
	"its":      "ɪts",
	"our":      "aʊɚ",
	"their":    "ðɛɹ",
	"this":     "ðɪs",
	"that":     "ðæt",
	"these":    "ðiz",
	"those":    "ðoʊz",
	"what":     "wʌt",
	"which":    "wɪtʃ",
	"who":      "hu",
	"whom":     "hum",
	"where":    "wɛɹ",
	"when":     "wɛn",
	"why":      "waɪ",
	"how":      "haʊ",
	"not":      "nɑt",
	"no":       "noʊ",
	"yes":      "jɛs",
	"to":       "tu",
	"of":       "ʌv",
	"in":       "ɪn",
	"on":       "ɑn",
	"at":       "æt",
	"by":       "baɪ",
	"for":      "fɔɹ",
	"with":     "wɪθ",
	"from":     "fɹʌm",
	"about":    "əbaʊt",
	"into":     "ɪntu",
	"through":  "θɹu",
	"after":    "æftɚ",
	"before":   "bɪfɔɹ",
	"between":  "bɪtwin",
	"under":    "ʌndɚ",
	"over":     "oʊvɚ",
	"up":       "ʌp",
	"down":     "daʊn",
	"out":      "aʊt",
	"off":      "ɔf",
	"if":       "ɪf",
	"then":     "ðɛn",
	"than":     "ðæn",
	"so":       "soʊ",
	"just":     "dʒʌst",
	"also":     "ɔlsoʊ",
	"very":     "vɛɹi",
	"well":     "wɛl",
	"here":     "hiɹ",
	"there":    "ðɛɹ",
	"now":      "naʊ",
	"only":     "oʊnli",
	"still":    "stɪl",
	"even":     "ivən",
	"again":    "əɡɛn",
	"back":     "bæk",
	"good":     "ɡʊd",
	"new":      "nu",
	"first":    "fɝst",
	"last":     "læst",
	"long":     "lɔŋ",
	"great":    "ɡɹeɪt",
	"little":   "lɪtəl",
	"own":      "oʊn",
	"other":    "ʌðɚ",
	"old":      "oʊld",
	"right":    "ɹaɪt",
	"big":      "bɪɡ",
	"high":     "haɪ",
	"small":    "smɔl",
	"large":    "lɑɹdʒ",
	"next":     "nɛkst",
	"early":    "ɝli",
	"young":    "jʌŋ",
	"important":"ɪmpɔɹtənt",
	"few":      "fju",
	"public":   "pʌblɪk",
	"same":     "seɪm",
	"able":     "eɪbəl",
	"say":      "seɪ",
	"said":     "sɛd",
	"get":      "ɡɛt",
	"make":     "meɪk",
	"go":       "ɡoʊ",
	"see":      "si",
	"know":     "noʊ",
	"take":     "teɪk",
	"come":     "kʌm",
	"think":    "θɪŋk",
	"look":     "lʊk",
	"want":     "wɑnt",
	"give":     "ɡɪv",
	"use":      "juz",
	"find":     "faɪnd",
	"tell":     "tɛl",
	"ask":      "æsk",
	"work":     "wɝk",
	"seem":     "sim",
	"feel":     "fil",
	"try":      "tɹaɪ",
	"leave":    "liv",
	"call":     "kɔl",
	"need":     "nid",
	"become":   "bɪkʌm",
	"keep":     "kip",
	"let":      "lɛt",
	"begin":    "bɪɡɪn",
	"show":     "ʃoʊ",
	"hear":     "hiɹ",
	"play":     "pleɪ",
	"run":      "ɹʌn",
	"move":     "muv",
	"live":     "lɪv",
	"believe":  "bɪliv",
	"hold":     "hoʊld",
	"bring":    "bɹɪŋ",
	"happen":   "hæpən",
	"write":    "ɹaɪt",
	"provide":  "pɹəvaɪd",
	"sit":      "sɪt",
	"stand":    "stænd",
	"lose":     "luz",
	"pay":      "peɪ",
	"meet":     "mit",
	"include":  "ɪnklud",
	"continue": "kəntɪnju",
	"set":      "sɛt",
	"learn":    "lɝn",
	"change":   "tʃeɪndʒ",
	"lead":     "lid",
	"understand":"ʌndɚstænd",
	"watch":    "wɑtʃ",
	"follow":   "fɑloʊ",
	"stop":     "stɑp",
	"create":   "kɹieɪt",
	"speak":    "spik",
	"read":     "ɹid",
	"spend":    "spɛnd",
	"grow":     "ɡɹoʊ",
	"open":     "oʊpən",
	"walk":     "wɔk",
	"win":      "wɪn",
	"offer":    "ɔfɚ",
	"remember": "ɹɪmɛmbɚ",
	"love":     "lʌv",
	"consider": "kənsɪdɚ",
	"appear":   "əpiɹ",
	"buy":      "baɪ",
	"wait":     "weɪt",
	"serve":    "sɝv",
	"die":      "daɪ",
	"send":     "sɛnd",
	"expect":   "ɪkspɛkt",
	"build":    "bɪld",
	"stay":     "steɪ",
	"fall":     "fɔl",
	"cut":      "kʌt",
	"reach":    "ɹitʃ",
	"kill":     "kɪl",
	"remain":   "ɹɪmeɪn",
	"hello":    "hɛloʊ",
	"okay":     "oʊkeɪ",
	"sure":     "ʃʊɹ",
	"thanks":   "θæŋks",
	"sorry":    "sɑɹi",
	"please":   "pliz",
}

// phonemeToID maps IPA characters to Kokoro token IDs.
// Based on Kokoro's vocabulary.
var phonemeToID = map[string]int64{
	" ":  1,
	"ɑ":  2,
	"æ":  3,
	"ʌ":  4,
	"ɔ":  5,
	"aʊ": 6,
	"aɪ": 7,
	"b":  8,
	"tʃ": 9,
	"d":  10,
	"ð":  11,
	"ɛ":  12,
	"ɝ":  13,
	"ɚ":  14,
	"eɪ": 15,
	"f":  16,
	"ɡ":  17,
	"h":  18,
	"i":  19,
	"ɪ":  20,
	"dʒ": 21,
	"k":  22,
	"l":  23,
	"m":  24,
	"n":  25,
	"ŋ":  26,
	"oʊ": 27,
	"ɔɪ": 28,
	"p":  29,
	"ɹ":  30,
	"s":  31,
	"ʃ":  32,
	"t":  33,
	"θ":  34,
	"u":  35,
	"ʊ":  36,
	"v":  37,
	"w":  38,
	"j":  39,
	"z":  40,
	"ʒ":  41,
	".":  42,
	",":  43,
	"ə":  44,
}
