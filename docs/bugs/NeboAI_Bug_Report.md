### Bug #4
**Date:** May 31, 2026  
**Severity:** Critical  
**Description:** Attaching any file or photo causes the entire message to fail sending - no message reaches Nebo  
**Steps to Reproduce:** 
1. Navigate to NeboAI.com
2. Open any loop's chat interface
3. Click attachment/image upload button
4. Select an image or file
5. Type a message (optional)
6. Press send/Enter
7. Observe behavior
**Expected Behavior:** Message with attachment should successfully send and appear in the chat thread  
**Actual Behavior:** Message fails to send entirely when an attachment is included - nothing appears in chat, no error message shown  
**Impact:** CRITICAL - Users cannot send any messages with attachments at all. This completely breaks the attachment feature.  
**Notes:** Need to investigate:
- Upload endpoint connectivity
- File size limits
- MIME type handling
- Frontend upload UI feedback
- Whether the send action is blocked or silently failing
- Network request inspection for errors

---

### Bug #5
**Date:** May 31, 2026  
**Severity:** High  
**Description:** Web browser tools are failing to load pages - returning empty content  
**Current State:** All web navigation and page reading attempts return 0 lines of content  
**Steps to Reproduce:** 
1. Call web search for any query
2. Attempt to navigate to a URL
3. Try to read the page
4. Observe empty results
**Expected Behavior:** Pages should load and display content normally  
**Actual Behavior:** All web actions return empty results despite successful connection  
**Impact:** Cannot browse websites, fetch data, or retrieve weather forecasts and other web-based information  
**Notes:** 
- Search queries execute but show only 1 result each (suspicious)
- Direct navigation to major sites (accuweather.com, weather.com, weather.gov) returns 0 lines
- API fetch calls also return empty
- Possible issues: CORS restrictions, JavaScript rendering requirements, network filtering, or tool misconfiguration
- Need to test basic connectivity and determine if this is a systematic issue

---

*Add more bugs below by updating this file with new entries.*
