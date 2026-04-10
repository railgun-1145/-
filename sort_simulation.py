import random
import time
import math
import sys

# 设置递归深度以防快速排序在极坏情况下溢出
sys.setrecursionlimit(2000)

class SortingSimulation:
    def __init__(self, size):
        self.size = size
        self.original_data = [random.randint(1, 100) for _ in range(size)]
        self.bogo_shuffles = 0
        self.quick_comparisons = 0
        self.quick_swaps = 0
        self.hybrid_shuffles = 0
        self.hybrid_partitions = 0

    def is_range_sorted(self, arr, low, high):
        for i in range(low, high):
            if arr[i] > arr[i+1]:
                return False
        return True

    def bogosort(self):
        data = list(self.original_data)
        start_time = time.time()
        self.bogo_shuffles = 0
        max_shuffles = 10000000 
        
        while not self.is_range_sorted(data, 0, len(data) - 1):
            random.shuffle(data)
            self.bogo_shuffles += 1
            if self.bogo_shuffles >= max_shuffles:
                return None, time.time() - start_time, False
                
        return data, time.time() - start_time, True

    def quicksort_wrapper(self):
        data = list(self.original_data)
        self.quick_comparisons = 0
        self.quick_swaps = 0
        start_time = time.time()
        self._quicksort(data, 0, len(data) - 1)
        return data, time.time() - start_time

    def _quicksort(self, arr, low, high):
        if low < high:
            pivot_index = self._partition(arr, low, high, 'quick')
            self._quicksort(arr, low, pivot_index - 1)
            self._quicksort(arr, pivot_index + 1, high)

    def _partition(self, arr, low, high, mode='quick'):
        pivot = arr[high]
        i = low - 1
        for j in range(low, high):
            if mode == 'quick':
                self.quick_comparisons += 1
            if arr[j] <= pivot:
                i += 1
                arr[i], arr[j] = arr[j], arr[i]
                if mode == 'quick':
                    self.quick_swaps += 1
        arr[i + 1], arr[high] = arr[high], arr[i + 1]
        if mode == 'quick':
            self.quick_swaps += 1
        return i + 1

    def hybrid_sort_wrapper(self):
        data = list(self.original_data)
        self.hybrid_shuffles = 0
        self.hybrid_partitions = 0
        start_time = time.time()
        self._hybrid_sort(data, 0, len(data) - 1)
        return data, time.time() - start_time

    def _hybrid_sort(self, arr, low, high):
        if low >= high:
            return

        # 优化策略：如果区间较小，使用“智能猴子排序”
        # 所谓“智能”：只打乱未排序的部分
        if (high - low + 1) <= 6:
            self._smart_bogosort(arr, low, high)
        else:
            # 较大区间：先尝试一次智能猴排（运气），不行再进行一次快排分区
            if not self.is_range_sorted(arr, low, high):
                # 尝试一次打乱
                self._smart_bogosort_once(arr, low, high)
                
                # 如果还是没排好，进行快速排序的分区步骤
                if not self.is_range_sorted(arr, low, high):
                    self.hybrid_partitions += 1
                    pivot_index = self._partition(arr, low, high, 'hybrid')
                    self._hybrid_sort(arr, low, pivot_index - 1)
                    self._hybrid_sort(arr, pivot_index + 1, high)

    def _smart_bogosort(self, arr, low, high):
        """完全随机打乱直到指定区间有序，但只打乱中间未排序部分"""
        while not self.is_range_sorted(arr, low, high):
            self._smart_bogosort_once(arr, low, high)

    def _smart_bogosort_once(self, arr, low, high):
        """识别未排序的边界并打乱一次"""
        # 找到左边第一个不符合升序的位置
        l_idx = low
        while l_idx < high and arr[l_idx] <= arr[l_idx + 1]:
            l_idx += 1
        
        # 找到右边第一个不符合升序的位置
        r_idx = high
        while r_idx > low and arr[r_idx] >= arr[r_idx - 1]:
            r_idx -= 1
        
        if l_idx < r_idx:
            # 提取未排序部分进行打乱
            sub = arr[l_idx : r_idx + 1]
            random.shuffle(sub)
            arr[l_idx : r_idx + 1] = sub
            self.hybrid_shuffles += 1

    def calculate_theoretical_probability(self):
        prob = 1.0 / math.factorial(self.size)
        return prob

def run_benchmarks():
    print(f"{'Size':<6} | {'Algorithm':<12} | {'Time (s)':<12} | {'Ops/Shuffles':<15} | {'Notes'}")
    print("-" * 85)
    
    for size in [5, 10, 15, 20]: 
        sim = SortingSimulation(size)
        
        # 1. Quicksort
        qs_result, qs_time = sim.quicksort_wrapper()
        print(f"{size:<6} | {'Quicksort':<12} | {qs_time:<12.6f} | {sim.quick_comparisons + sim.quick_swaps:<15} | {'Pure Efficiency'}")
        
        # 2. Hybrid Sort (Combine Bogo + Quick)
        hs_result, hs_time = sim.hybrid_sort_wrapper()
        print(f"{'':<6} | {'Hybrid Sort':<12} | {hs_time:<12.6f} | {hs_time/qs_time if qs_time>0 else 0:<15.2f} | {'S: '+str(sim.hybrid_shuffles)+' P: '+str(sim.hybrid_partitions)}")
        
        # 3. Bogosort (Only for small size)
        if size <= 10:
            bs_result, bs_time, bs_success = sim.bogosort()
            status = f"{sim.bogo_shuffles}" if bs_success else "TIMEOUT"
            print(f"{'':<6} | {'Bogosort':<12} | {bs_time:<12.6f} | {status:<15} | {'Theoretical Min Prob'}")
        
        print("-" * 85)

if __name__ == "__main__":
    run_benchmarks()
