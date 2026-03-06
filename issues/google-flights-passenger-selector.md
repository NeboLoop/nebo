# Google Flights Passenger Selector Bug

## Issue
The browser automation tool cannot reliably set the passenger count on Google Flights.

## Description
When trying to add passengers via the passenger selector dialog:
1. The dialog opens correctly
2. Clicking "Add adult" and "Add child aged 2 to 11" buttons
3. The dialog closes unexpectedly OR doesn't register the clicks properly
4. The "Done" button either doesn't work or changes aren't applied

## Expected Behavior
- Be able to click "Add adult" multiple times to increase adult count
- Be able to click "Add child aged 2 to 11" to add children
- Click "Done" to apply the passenger count

## Actual Behavior
- Dialog closes before count is finalized
- Clicks on add buttons may not register
- Final passenger count shows incorrect number (e.g., shows 6 instead of 7)

## Steps to Reproduce
1. Navigate to Google Flights
2. Enter origin (Provo) and destination (San Diego area)
3. Select dates
4. Click on passenger count button to open dialog
5. Try to add adults and children
6. Click "Done"

## Current State
- Shows "6 passengers" but need 7 (4 adults ages 12+, 1 child age 11)
- Multiple attempts to add passengers via automation have failed

## Impact
Cannot automate flight searches with specific passenger counts on Google Flights.

## Browser Extension Status
- Extension appears connected
- Other interactions (click, fill, scroll) work fine
- This is specific to the passenger selector dialog behavior
