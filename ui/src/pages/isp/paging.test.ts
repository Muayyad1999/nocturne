import { describe, expect, it } from 'vitest';
import { back, cursorAfter, cursorParam, firstRow, forward } from './paging';

const rows = (count: number) =>
  Array.from({ length: count }, (_, index) => ({ id: `row-${index + 1}` }));

describe('a page continues after a row, not after a count of rows', () => {
  it('asks for the page after the last row of a full page', () => {
    expect(cursorAfter(rows(50), 50)).toBe('row-50');
  });

  it('knows a page that is not full is the last one', () => {
    expect(cursorAfter(rows(13), 50)).toBeNull();
    expect(cursorAfter([], 50)).toBeNull();
  });

  it('sends no cursor on the first page, and the last one walked after it', () => {
    expect(cursorParam([])).toEqual({});
    expect(cursorParam(['row-50'])).toEqual({ cursor: 'row-50' });
    expect(cursorParam(['row-50', 'row-100'])).toEqual({ cursor: 'row-100' });
  });

  it('goes back exactly one page, and no further than the first', () => {
    const walked = forward(forward([], 'row-50'), 'row-100');
    expect(walked).toEqual(['row-50', 'row-100']);
    expect(back(walked)).toEqual(['row-50']);
    expect(back(back(back(walked)))).toEqual([]);
  });

  it('says where a page is by how many came before it', () => {
    expect(firstRow([], 50)).toBe(1);
    expect(firstRow(['row-50'], 50)).toBe(51);
    expect(firstRow(['row-50', 'row-100'], 50)).toBe(101);
  });

  it('never asks for a page by the number of rows already seen', () => {
    // What the cursor used to be: the engine now refuses it.
    const walked = forward([], cursorAfter(rows(50), 50) ?? '');
    expect(cursorParam(walked).cursor).not.toMatch(/^\d+$/);
  });
});
